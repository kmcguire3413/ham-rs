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


trait Block {
    fn work(&mut self, xin: &Vec<Complex<f32>>);
}

#[test]
fn test_server() {
    let th = thread::spawn(move || {
        use muds::block::net::ControlInfo;
        use std::sync::mpsc::{channel, Receiver, Sender};
    
        let server = muds::block::net::Server::new("0.0.0.0:1055").unwrap();
        
        let (tx, rx) = channel::<(u64, u8, u64, u8)>();
        
        println!("server started");
        loop {
            println!("waiting on message from server");
            match server.read().unwrap() {
                ControlInfo::ClientHello { luid, client } => {
                    println!("client hello {}", luid);
                    tx.send((luid, 0, 0, 0));
                },
                ControlInfo::ClientBye { luid, client } => {
                    println!("client byte {}", luid);
                    tx.send((luid, 1, 0, 0));
                },
                ControlInfo::ClientData { luid, client } => {
                    println!("client data {}", luid);
                    let buf = client.read().unwrap();
                    let chk: u8 = 0;
                    for x in 0..buf.len() {
                        chk = chk ^ buf[x];
                    }
                    tx.send((luid, 2, buf.len(), chk));
                },
                ControlInfo::ClientFull { luid, client } => {
                    println!("client full {}", luid);
                    tx.send((luid, 3, 0, 0));
                }
            }
        }
    });
    
    
    
    th.join();
}

pub mod muds {
    pub mod block {
        pub mod net {
            use std::sync::{Arc, Mutex, Condvar};
            use std::net::{TcpListener, TcpStream};
            use std::thread;      
            use std::collections::{VecDeque, HashMap};
            use std::sync::mpsc::{Sender, Receiver, channel, RecvError};
            use std;
            
            use std::io::{Read, Write};
            
            pub struct Client {
                stream:      TcpStream,
                luid:        u64,
                buffer:      VecDeque<Vec<u8>>,
                ctrltx:      Sender<ControlInfo>,
                waitsz:      usize,
                waitlimit:   usize,
                fullsignal:  Arc<(Mutex<bool>, Condvar)>,
            }
            
            impl Client {
                pub fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                    self.stream.write(buf)
                }
                pub fn read(&mut self) -> Option<Vec<u8>> {
                    let mut mustnotify = false;
                    if self.buffer.len() >= self.waitlimit {
                        mustnotify = true;
                    }
                    match self.buffer.pop_front() {
                        Option::Some(v) => {
                            self.waitsz -= v.len();
                            if mustnotify {
                                // Obviously, the wait is happening now or is 
                                // about to happen.
                                {
                                    let mut started = self.fullsignal.0.lock().unwrap();
                                    *started = true;
                                }
                                self.fullsignal.1.notify_one();                            
                            }
                            Option::Some(v)
                        },
                        Option::None => Option::None,
                    }
                }
                pub fn get_luid(&self) -> u64 {
                    self.luid
                }
                pub fn can_read(&self) -> bool {
                    self.buffer.len() > 0
                }
            }
            
            pub enum ControlInfo {
                ClientHello { luid: u64, client: Arc<Mutex<Client>> },
                ClientBye { luid: u64, client: Arc<Mutex<Client>> },
                ClientData { luid: u64, client: Arc<Mutex<Client>> },
                ClientFull { luid: u64, client: Arc<Mutex<Client>> },
            }
        
            pub struct Server {
                strms:      Arc<Mutex<HashMap<u64, Arc<Mutex<Client>>>>>,
                ctrlrx:     Receiver<ControlInfo>,
            }
            
            impl Server {
                pub fn write(&self, luid: u64, buf: &[u8]) -> std::io::Result<usize> {
                    let strms = self.strms.lock().unwrap();
                    let mut lck = strms.get(&luid).unwrap().lock().unwrap();
                    lck.write(buf)
                }
                
                pub fn read(&self) -> Result<ControlInfo, RecvError> {
                    self.ctrlrx.recv()
                }        
            
                pub fn new(addr: &str) -> Option<Server>  {                    
                    let srv = TcpListener::bind(addr);
                    
                    // TODO: add method to shutdown this server.. maybe using
                    //       a client whom connections with a special message
                    match srv {
                        Result::Ok(srv) => {
                            let strms: Arc<Mutex<HashMap<u64, Arc<Mutex<Client>>>>> = Arc::new(Mutex::new(HashMap::new()));
                            let strms_clone = strms.clone();
                            let (ctrltx, ctrlrx) = channel::<ControlInfo>();                        
                            thread::spawn(move || {
                                let mut luid: u64 = 100;
                                for stream in srv.incoming() {
                                    match stream {
                                        Result::Ok(stream) => {
                                            luid += 1;
                                            stream.set_read_timeout(Option::None);
                                            stream.set_write_timeout(Option::None);
                                            let mut stream_clone = stream.try_clone().unwrap();
                                            let client = Arc::new(Mutex::new(Client {
                                                stream:     stream,
                                                luid:       luid,
                                                buffer:     VecDeque::new(),
                                                ctrltx:     ctrltx.clone(),   
                                                waitsz:    0,
                                                waitlimit:  1024 * 1024 * 16,
                                                fullsignal: Arc::new((Mutex::new(false), Condvar::new())),                                                                                          
                                            }));
                                            let ctrltx_clone = ctrltx.clone();
                                            let client_clone = client.clone();
                                            let mut lck = strms.lock().unwrap();
                                            lck.insert(luid, client);
                                            ctrltx.send(ControlInfo::ClientHello { luid: luid, client: client_clone.clone() });
                                            thread::spawn(move || {
                                                let bufsz = 2048;
                                                let mut buf: Vec<u8>;
                                                loop {
                                                    buf = Vec::with_capacity(bufsz);
                                                    unsafe {
                                                        buf.set_len(bufsz);
                                                    }                                                
                                                    let rsz = match stream_clone.read(buf.as_mut_slice()) {
                                                        Result::Ok(v) => v,
                                                        Result::Err(_) => {
                                                            // Terminate this reader thread.
                                                            ctrltx_clone.send(ControlInfo::ClientBye { luid: luid, client: client_clone.clone() });
                                                            return;
                                                        },
                                                    };
                                                    
                                                    println!("DATA!!!");                          
                                                    
                                                    if rsz < 1 {
                                                        continue;
                                                    }
                                                    unsafe {
                                                        buf.set_len(rsz);
                                                    }
                                                    
                                                    let mut fullsignal: Option<Arc<(Mutex<bool>, Condvar)>> = Option::None;
                                                    {
                                                        // Only lock long enough to put data into
                                                        // the buffer.
                                                        let mut lck = client_clone.lock().unwrap();
                                                        if lck.waitsz < lck.waitlimit {
                                                            lck.waitsz += buf.len();
                                                            lck.buffer.push_back(buf);
                                                            if lck.buffer.len() == 1 {
                                                                lck.ctrltx.send(ControlInfo::ClientData { luid: luid, client: client_clone.clone() });
                                                            }
                                                            if lck.waitsz >= lck.waitlimit {
                                                                // Get setup to stop reading from the socket.
                                                                lck.ctrltx.send(ControlInfo::ClientFull { luid: luid, client: client_clone.clone() });
                                                                fullsignal = Option::Some(lck.fullsignal.clone());    
                                                            }
                                                        } 
                                                    }
                                                    
                                                    // The point here is to stop reading from the socket which will
                                                    // cause the TCP stream receive window to decrease to zero if needed
                                                    // until we can resume reading from the socket. This is needed to
                                                    // prevent the remote end from exhausting our memory. We can not simply
                                                    // spin here and eat CPU either so we need to block for a signal. If by
                                                    // the time we wake the socket is dead then we shall know gracefully and
                                                    // exit gracefully up above this point in the loop.
                                                    match fullsignal {
                                                        Option::Some(fullsignal) => {
                                                            let mut started = fullsignal.0.lock().unwrap();
                                                            while !*started {
                                                                started = fullsignal.1.wait(started).unwrap();
                                                            }
                                                        },
                                                        Option::None => (),
                                                    }
                                                }
                                            });
                                        },
                                        Result::Err(err) => (),
                                    }
                                }
                            });
                            Option::Some(Server {
                                strms:      strms_clone,
                                ctrlrx:     ctrlrx,
                            })
                        },
                        Result::Err(err) => Option::None,                   
                    }
                } 
            }
        }
        mod directory {
            pub fn net() {
                
            }
            
            pub fn sys() {
            }
            
            pub fn link_to(sysid: Vec<u8>, compid: Vec<u8>, portid: Vec<u8>) {
            }
            
            pub fn link_from(compid: Vec<u8>, portid: Vec<u8>) {
            }
        }
        
        pub fn usrp() {            
        }
    }
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


