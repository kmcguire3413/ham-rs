#![allow(dead_code)]
#![allow(unused_imports)]
#![feature(path_ext)]
#![feature(convert)]
#![feature(deque_extras)]
#![feature(heap_api)]
#![feature(alloc)]
#![allow(non_camel_case_types)]
extern crate lodepng;
extern crate byteorder;
extern crate alsa;
extern crate num;
extern crate time;
extern crate libc;
extern crate alloc;

use std::cmp::Ordering;
use std::io::{SeekFrom, Seek, Cursor, Read};
use std::path::{Path};
use std::fs::{File, PathExt};
use byteorder::{ReadBytesExt, WriteBytesExt, LittleEndian};
use std::mem;
use std::str::FromStr;
use std::thread;
use std::sync::{Arc, Mutex};

pub mod usrp;
pub mod algos;
pub mod dsp;

pub use algos::SignalMap;
pub use algos::mcguire_smde;

pub use dsp::Complex;
pub use dsp::FMDemod;
pub use dsp::wavei8write;
pub use dsp::FileSource;

pub use usrp::USRPSource;

pub struct Transmission {
    pub freq:       f64,
    pub buf:        Vec<f32>,
}

pub struct MonitorSpec {
    pub freq:       f64,
}

/// Internally used monitor structure.
struct Monitor {
    freq:       f64,
    offset:     f64,
    demod:      FMDemod,
    buf:        Vec<f32>,
    dead:       isize,
}

pub fn router(rtrans: Arc<Mutex<Vec<Transmission>>>, targets: Vec<MonitorSpec>) {        
    println!("[ham-router] initializing");
    
    // Let us determine the actual spread of these frequencies so we know
    // we sample rate we need to run at in order to capture them.
    let mut target_min = std::f64::MAX;
    let mut target_max = std::f64::MIN;
    for x in 0..targets.len() {
        if targets[x].freq > target_max {
            target_max = targets[x].freq;
        }
        
        if targets[x].freq < target_min {
            target_min = targets[x].freq;
        }
    }
    
    // Add 100khz on both sides just to be safe, and this will
    // be our sample rate.
    let mut sps = (target_max - target_min) + 200000.0 * 2.0;
    
    if sps < 4000000.0 {
        sps = 4000000.0;
    }
    

    // Set our center frequency in the middle of the spread.
    let freq_center = target_min + (target_max - target_min) * 0.5;
    
    println!("[ham-router] setting center frequency to {}", freq_center);
    println!("[ham-router] setting sample rate to {}", sps);
    
    // This forms a nice filter for 15khz FM.
    let taps: Vec<f32> = vec![0.44378024339675903f32, 0.9566655158996582, 1.4999324083328247, 2.0293939113616943, 2.499887466430664, 2.8699963092803955, 3.106461763381958, 3.1877646446228027, 3.106461763381958, 2.8699963092803955, 2.499887466430664, 2.0293939113616943, 1.4999324083328247, 0.9566655158996582, 0.44378024339675903];

    let mut monitors: Vec<Monitor> = Vec::new();

    let decim = (sps / 400000.0).floor() as usize;
    
    println!("decim set to {}", decim);   
    
    for x in 0..targets.len() {
        println!("offset:{} frequency:{}", freq_center - targets[x].freq, targets[x].freq);
        monitors.push(Monitor {
            freq:       targets[x].freq,
            offset:     freq_center - targets[x].freq,
            demod:      FMDemod::new(sps, decim, freq_center - targets[x].freq, 15000.0, taps.clone(), 3),
            buf:        Vec::new(),  
            dead:       1,
        });
    }

    //     
    //let mut fmdemod0 = FMDemod::new(sps, 10, -440000.0, 15000.0, taps, 3);
    
    //let mut buf: Vec<f32> = Vec::new();
    
    //let mut alsa = dsp::Alsa::new(16000);

    // At the moment the gain is locked at 70dB since my setup is currently
    // using a very low gain antenna. However, in the future I can look at
    // auto adjusting the gain lower if it causes over-saturation.
    let mut ausrp = USRPSource::new(sps, freq_center, 1.0);
    let mut usrp = ausrp.lock().unwrap();
    
    // A debugging source that mimics the USRP as a source.    
    //let mut usrp = FileSource::new(String::from_str("/home/kmcguire/Projects/radiowork/usbstore/recording01").unwrap());   
    
    // This just keeps an instrumental tracking of the number of samples
    // that have been processed.
    let mut total_samps = 0;
            
    let gst = time::precise_time_ns() as f64 / 1000.0 / 1000.0 / 1000.0;
    
    println!("[ham-router] capturing broadband signal..");

    let mut avgpwr = 0f64;
    let mut avgcnt = 0usize;
    let mut curgain = 1.0f64;
    let mut maxgain = 50.0f64;

    loop {
        let mut ibuf = usrp.recv();
        
        // Try to establish AGC.
        for x in 0..ibuf.len() {
            let spwr = (ibuf[x].i * ibuf[x].i + ibuf[x].q * ibuf[x].q).sqrt();
            avgpwr += spwr as f64;
            avgcnt += 1;
        }
        
        if avgcnt > 500000 {
            avgpwr = avgpwr / avgcnt as f64;
            if avgpwr > 0.20 {
                curgain -= 1.0;
                if curgain < 1.0 {
                    curgain = 1.0;
                }
                usrp.set_rx_gain(curgain);
                println!("gain decreased to {} with avg pwr {}", curgain, avgpwr);
            }
            if avgpwr < 0.03 {
                curgain += 1.0;
                if curgain > maxgain {
                    curgain = maxgain;
                }
                usrp.set_rx_gain(curgain);
                println!("gain increased to {} with avg pwr {}", curgain, avgpwr);
            }
            avgcnt = 0;
            avgpwr = 0.0;
        }
                
        total_samps += ibuf.len();

        let st = time::precise_time_ns() as f64 / 1000.0 / 1000.0 / 1000.0;

        for mon in monitors.iter_mut() {        
        
            let mut out = mon.demod.work(&ibuf);

            if total_samps > 4000000 {            
                println!("freq:{} sq:{} dead:{}", mon.freq, mon.demod.sq, mon.dead);
            }   
        
            for x in 0..out.len() {
                if out[x].abs() > 0.0 {
                    mon.dead -= 1;
                } else {
                    mon.dead += 1;
                }
            
                if mon.dead < -8000 {
                    mon.dead = -8000;
                }
                
                if mon.dead > 30 {
                    mon.dead = 30;
                }
                
                // If we have activity then start pushing the audio output
                // samples into our monitor buffer. Once we see no activity
                // then evaluate if it contains enough to be considered a
                // transmission and if so then place it into the output queue
                // and prepare for the next transmission.
                if mon.dead < 0 {
                    mon.buf.push(out[x]);     
                } else {
                    if mon.buf.len() / 16000 > 2 {
                        // Place the transmission into the output buffer. The locking
                        // provides the synchronization to support an external thread
                        // accessing the buffer concurrently.
                        println!("[ham-router] placed transmission of {} seconds into output buffer", mon.buf.len() as f32 / 16000.0);
                        let mut tout = rtrans.lock().unwrap();

                        // Since we have ownership rules and rules that keep something from
                        // being uninitialized we must create a fresh buffer and swap it out
                        // with the buffer that we wish to put into the queue.
                        let mut tmpbuf: Vec<f32> = Vec::new();
                        std::mem::swap(&mut tmpbuf, &mut mon.buf);                        
                        
                        tout.push(Transmission {
                            freq:       mon.freq,
                            buf:        tmpbuf,
                        });
                    } else {
                        mon.buf.clear();
                    }
                }
            }        
        }
                
        if total_samps > 4000000 {
            total_samps = 0;
        }
        
        //println!("done {}", (time::precise_time_ns() - st) as f64 / 1000.0 / 1000.0 / 1000.0);
        //println!("wavbuf:{}", buf.len());
        //println!("total_samps:{} time:{}", total_samps, st as f64 / 1000.0 / 1000.0 / 1000.0);
    }
    
    //let gsec = (time::precise_time_ns() as f64 - gst as f64) / 1000.0 / 1000.0 / 1000.0;
    //println!("total_samps:{} total_time:{} samps_per_second:{}", total_samps, gsec, total_samps as f64 / gsec); 
    //println!("total_samps/sps:{}", total_samps as f32 / 4000000.0);    
    //wavei8write(String::from_str("tmp.wav").unwrap(), 16000, &buf);
    //println!("done");
}


