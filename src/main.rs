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

mod usrp;
mod algos;
mod dsp;

use algos::SignalMap;
use algos::mcguire_smde;

use dsp::Complex;
use dsp::FMDemod;
use dsp::wavei8write;
use dsp::FileSource;

use usrp::USRPSource;

fn main() {        
    let taps: Vec<f32> = vec![0.44378024339675903f32, 0.9566655158996582, 1.4999324083328247, 2.0293939113616943, 2.499887466430664, 2.8699963092803955, 3.106461763381958, 3.1877646446228027, 3.106461763381958, 2.8699963092803955, 2.499887466430664, 2.0293939113616943, 1.4999324083328247, 0.9566655158996582, 0.44378024339675903];
    
    let mut fmdemod0 = FMDemod::new(4000000.0, 10, -450000.0, 15000.0, taps, 3);

    println!("processing");
    
    let mut buf: Vec<f32> = Vec::new();
    
    let mut alsa = dsp::Alsa::new(16000);

    let mut ausrp = USRPSource::new(4000000.0, 146000000.0, 10.0);
    let mut usrp = ausrp.lock().unwrap();
    //let mut usrp = FileSource::new(String::from_str("/home/kmcguire/Projects/radiowork/usbstore/recording01").unwrap());   
    
    println!("usrp created");
    
    let mut total_samps = 0;

    let mut sum: Complex<f32> = Complex { i: 0.0, q: 0.0 };    
    let mut sumcnt = 0usize;
    
    let gst = time::precise_time_ns();
    //while buf.len() < 16000 * 15 {
    loop {
        let mut ibuf = usrp.recv();
        for x in 0..ibuf.len() {
            sum.i += ibuf[x].i;
            sum.q += ibuf[x].q;
            sumcnt += 1;
        }

        if (sumcnt > 4000000) {
            sum.i = sum.i / sumcnt as f32;
            sum.q = sum.q / sumcnt as f32;
            sumcnt = 1;
        }        
        
        for x in 0..ibuf.len() {
            ibuf[x].i = ibuf[x].i - sum.i / sumcnt as f32;
            ibuf[x].q = ibuf[x].q - sum.q / sumcnt as f32;
        }
        
        total_samps += ibuf.len();
        //println!("decoding block");
        let st = time::precise_time_ns();
        let mut out = fmdemod0.work(&ibuf);

        //println!("out.len:{}", out.len());   

        for x in 0..out.len() {
            std::io::stderr().write_f32::<LittleEndian>(out[x] * 12.5);
        }        
        
        //println!("done {}", (time::precise_time_ns() - st) as f64 / 1000.0 / 1000.0 / 1000.0);
        //buf.append(&mut out);
        
        //if buf.len() > 4000 {
        //    alsa.write(&buf);
        //    buf.clear();
        //}
        
        //println!("wavbuf:{}", buf.len());
        //println!("total_samps:{} time:{}", total_samps, st as f64 / 1000.0 / 1000.0 / 1000.0);
    }
    
    let gsec = (time::precise_time_ns() as f64 - gst as f64) / 1000.0 / 1000.0 / 1000.0;
    println!("total_samps:{} total_time:{} samps_per_second:{}", total_samps, gsec, total_samps as f64 / gsec); 
    
    println!("total_samps/sps:{}", total_samps as f32 / 4000000.0);
    
    wavei8write(String::from_str("tmp.wav").unwrap(), 16000, &buf);
    
    println!("done");
}


