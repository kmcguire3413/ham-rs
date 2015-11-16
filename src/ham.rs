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

pub fn router(rtrans: Arc<Mutex<Vec<Vec<f32>>>>) {        
    println!("[ham-router] initializing");
    let taps: Vec<f32> = vec![0.44378024339675903f32, 0.9566655158996582, 1.4999324083328247, 2.0293939113616943, 2.499887466430664, 2.8699963092803955, 3.106461763381958, 3.1877646446228027, 3.106461763381958, 2.8699963092803955, 2.499887466430664, 2.0293939113616943, 1.4999324083328247, 0.9566655158996582, 0.44378024339675903];

    let sps = 4000000.0;    
    
    let mut fmdemod0 = FMDemod::new(sps, 10, -440000.0, 15000.0, taps, 3);
    
    let mut buf: Vec<f32> = Vec::new();
    
    let mut alsa = dsp::Alsa::new(16000);

    let mut ausrp = USRPSource::new(sps, 146000000.0, 70.0);
    let mut usrp = ausrp.lock().unwrap();
    //let mut usrp = FileSource::new(String::from_str("/home/kmcguire/Projects/radiowork/usbstore/recording01").unwrap());   
    
    let mut total_samps = 0;

    let mut sum: Complex<f32> = Complex { i: 0.0, q: 0.0 };    
    let mut sumcnt = 0usize;

    let mut dead: isize = 0;
    
    let mut skip: usize = 0;
    
    let gst = time::precise_time_ns() as f64 / 1000.0 / 1000.0 / 1000.0;
    
    println!("[ham-router] listening to broadband signal");

    loop {
        let mut ibuf = usrp.recv();
                
        total_samps += ibuf.len();
        let st = time::precise_time_ns() as f64 / 1000.0 / 1000.0 / 1000.0;
        let mut out = fmdemod0.work(&ibuf);   
        
        for x in 0..out.len() {
            if out[x].abs() > 0.0 {
                dead -= 1;
            } else {
                dead += 1;
            }
            
            if dead < -30 {
                dead = -30;
            }
            
            if dead > 30 {
                dead = 30;
            }
            
            if dead < 0 {
                buf.push(out[x]);     
            } else {
                if buf.len() / 16000 > 2 {
                    //println!("saved {}", buf.len() / 16000);
                    //wavei8write(format!("rec_{}.wav", (st - gst).floor()), 16000, &buf);
                    
                    // Place the transmission into the output buffer. The locking
                    // provides the synchronization to support an external thread
                    // accessing the buffer concurrently.
                    println!("[ham-router] placed transmission of {} seconds into output buffer", buf.len() as f32 / 16000.0);
                    let mut out = rtrans.lock().unwrap();
                    out.push(buf);
                    
                    buf = Vec::new();
                } else {
                    buf.clear();
                }
            }
        }        
        
        if total_samps > 4000000 * 2 {
            total_samps = 0;
            println!("seq:{}", fmdemod0.sq);
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


