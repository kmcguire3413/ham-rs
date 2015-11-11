use std::path::{Path};
use std::fs::{File, PathExt};
use byteorder::{WriteBytesExt, LittleEndian};

use std::ffi::CString;
use alsa::{Direction, ValueOr};
use alsa::pcm::{PCM, HwParams, Format, Access, State};

use std::collections::VecDeque;

use num;

#[derive(Clone)]
pub struct Complex<T> {
    pub i:              T,
    pub q:              T,
}

impl<T: num::traits::Float> Complex<T> {
    pub fn mul(&mut self, a: &Complex<T>) {
        let i = self.i * a.i - self.q * a.q;
        let q = self.i * a.q + self.q * a.i;
        self.i = i;
        self.q = q;
    }

}

pub struct Alsa {
    sps:            u32,
    pcm:            PCM,
    consumed:       usize,
}

impl Alsa {
    pub fn new(sps: u32) -> Alsa {
        // Open default playback device
        let pcm = PCM::open(&*CString::new("default").unwrap(), Direction::Playback, false).unwrap();
        
        {
            let hwp = HwParams::any(&pcm).unwrap();
            hwp.set_channels(1).unwrap();
            hwp.set_rate(sps, ValueOr::Nearest).unwrap();
            hwp.set_format(Format::s16()).unwrap();
            hwp.set_access(Access::RWInterleaved).unwrap();
            pcm.hw_params(&hwp).unwrap();
        }
            
        Alsa { sps: sps, pcm: pcm, consumed: 0 }
    }
    
    pub fn get_consumed(&self) -> usize {
        self.consumed
    }
    
    pub fn write(&mut self, buf: &Vec<i16>) {
        {
            let io = self.pcm.io_i16().unwrap();
            self.consumed += buf.len();
            io.writei(buf);
        }
        if self.pcm.state() != State::Running { self.pcm.start().unwrap() };
        self.pcm.drain().unwrap();
    }     
}