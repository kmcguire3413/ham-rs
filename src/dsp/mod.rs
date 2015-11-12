use std::path::{Path};
use std::fs::{File, PathExt};

use byteorder::{ReadBytesExt, WriteBytesExt, LittleEndian};

use std::ffi::CString;

use alsa::{PCM, Stream, Mode, Format, Access, Prepared};

use std::collections::VecDeque;
use std;
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
    pcm:            PCM<Prepared>,
}

impl Alsa {
    pub fn new(sps: u32) -> Alsa {
        let pcm = PCM::open("default", Stream::Playback, Mode::Blocking).unwrap();
        let mut pcm = pcm.set_parameters(Format::FloatLE, Access::Interleaved, 1, sps as usize).ok().unwrap();
            
        Alsa { sps: sps, pcm: pcm }
    }
        
    pub fn write(&mut self, buf: &Vec<f32>) {
        self.pcm.write_interleaved(&buf).unwrap();
    }     
}

pub fn buildsine(freq: f64, sps: f64, amp: f32) -> Option<Vec<Complex<f32>>> {
    if freq * 4.0 > sps {
        return Option::None;
    }
    
    // How many of our smallest units of time represented
    // by a sample do we need for a full cycle of the
    // frequency.
    let timepersample = 1.0f64 / sps as f64;
    let units = ((1.0 / freq).abs() / timepersample).ceil() as usize;
    
    println!("timepersample:{} freqfullwave:{}", 
        timepersample,
        1.0 / freq
    );
    
    let mut out: Vec<Complex<f32>> = Vec::new();

    println!("units:{}", units);
    
    for x in 0..units * 100 {
        let curtime = (x as f64) * timepersample;
        out.push(Complex {
            i:  (curtime * freq * std::f64::consts::PI * 2.0).cos() as f32 * amp,
            q:  (curtime * freq * std::f64::consts::PI * 2.0).sin() as f32 * amp,
        });
    }
    
    Option::Some(out)
}

pub struct FMDemod {
    sps:        f64,
    offset:     f64,
    bw:         f32,
    q0:         usize,
    q1:         usize,
    li:         f32,
    lq:         f32,
    ifs:        Vec<Complex<f32>>,
    ifsndx:     usize,
    rsum:       f32,
    tapi:       usize,
    tapvi:      Vec<f32>,
    tapvq:      Vec<f32>,
    taps:       Vec<f32>,
    decim:      usize,
    curslack:   f32,
    maxphase:   f32,
    slack:      f32,
    audiodecim: usize,
    sq:         usize,
    devsqlimit: usize,
}

impl FMDemod {
    pub fn new(sps: f64, decim: usize, offset: f64, bw: f32, taps: Vec<f32>, devsqlimit: usize) -> FMDemod {
        let ifs = buildsine(offset, sps, 5.0).unwrap();
        
        let mut tapvi: Vec<f32> = Vec::new();
        let mut tapvq: Vec<f32> = Vec::new();
        
        for x in 0..taps.len() {
            tapvi.push(0.0);
            tapvq.push(0.0);
        }

        // If the decimation is not set perfectly then we will have
        // a fractional part and we need to insert some padding when
        // it reaches a value representing a whole output sample.
        let actual_audio_decim = (sps / (decim as f64) / 16000.0) as f32;
        // Make sure that slack is not >= 1.0 
        let slack = actual_audio_decim.fract();        
        let pract_audio_decim = actual_audio_decim.ceil() as usize;
        let fmaxphaserot = ((std::f64::consts::PI * 2.0f64) / (sps / (decim as f64))) * bw as f64;
        
        println!("slack:{} pract_audio_decim:{} decim:{} actual_audio_decim:{}",
            slack, pract_audio_decim, decim, actual_audio_decim
        );        
            
    	FMDemod { 
    	   devsqlimit: devsqlimit,
    	   maxphase:   fmaxphaserot as f32,
    	   audiodecim: pract_audio_decim,
    	   slack:      slack,
    	   sps:        sps, 
    	   offset:     offset, 
    	   bw:         bw, 
    	   li:         0.0, 
    	   lq:         0.0,
    	   q0:         0,
    	   q1:         0,
    	   ifs:        ifs,
    	   ifsndx:     0,
    	   rsum:       0.0,
    	   tapi:       0,
    	   tapvi:      tapvi,
    	   tapvq:      tapvq,
    	   taps:       taps,
    	   decim:      decim,
    	   curslack:   0.0,
    	   sq:         0,
    	}
    }
    
    pub fn work(&mut self, stream: &Vec<Complex<f32>>) -> Vec<f32> {
        let mut buf: Vec<f32> = Vec::with_capacity(stream.len() / self.decim / self.audiodecim); 
            
        for x in 0..stream.len() {
            let mut s = stream[x].clone();            
                        
            s.mul(&self.ifs[self.ifsndx]);
            
            self.ifsndx = if self.ifsndx + 1 >= self.ifs.len() {
                0
            } else {
                self.ifsndx + 1
            };               

            if self.q0 == self.decim {
                self.q0 = 0;

                if self.curslack >= 1.0 {
                    // Hopefully, the slack is < 1.0
                    self.curslack = self.curslack.fract();
                    self.li = s.i;
                    self.lq = s.q;
                    continue;
                }                 
                                
                self.tapvi[self.tapi as usize] = s.i;
                self.tapvq[self.tapi as usize] = s.q;
                
                let mut si = 0.0f32;
                let mut sq = 0.0f32;
                
                for ti in 0..self.taps.len() {
                    let off = if (ti > self.tapi) { self.taps.len() - (ti - self.tapi)} else { self.tapi - ti };
                    si += self.tapvi[off] * self.taps[ti];
                    sq += self.tapvq[off] * self.taps[ti];
                }
                
                self.tapi += 1;
                if self.tapi >= self.taps.len() {
                    self.tapi = 0;
                }        
                
                s.i = si;   
                s.q = sq;

                let mut a = s.i.atan2(s.q);
                let mut b = self.li.atan2(self.lq);
                                
                let mut r = 0f32;
                
                r = a - b;
                
                if r > std::f32::consts::PI {
                    r = std::f32::consts::PI * 2.0 + r;
                }                
                                
                // This limits sharp impulses where spikes have slipped
                // through our taps filter.
                if r.abs() < self.maxphase {
                    self.rsum += r;
                    self.sq -= 1;
                    if self.sq < 1 {
                        self.sq = 1;
                    }
                } else {
                    self.sq += 1;
                    if self.sq > 10 {
                        self.sq = 30;
                    }
                }
                
                self.q1 += 1; 
                if self.q1 == self.audiodecim {
                    self.q1 = 0;
                    
                    // Track how much we are off on the audio output
                    // due to decimation of the input stream by a value
                    // that causes our audio decimation to have a fractional
                    // part.
                    self.curslack += self.slack;
                    
                    self.rsum /= self.audiodecim as f32;
                    if self.sq > self.devsqlimit {
                        buf.push(0.0);
                    } else {      
                        buf.push(self.rsum / (self.sq as f32));
                    }
                    
                    self.rsum = 0f32;                    
                } 
            }
             
            self.li = s.i;
            self.lq = s.q;   
        
            self.q0 += 1;
        }    
        
        // Return the buffer containing the demodulated data.
        buf
    }
}

#[inline]
fn u16tou8ale(v: u16) -> [u8; 2] {
    [
        v as u8,
        (v >> 8) as u8,
    ]
}

// little endian
#[inline]
fn u32tou8ale(v: u32) -> [u8; 4] {
    [
        v as u8,
        (v >> 8) as u8,
        (v >> 16) as u8,
        (v >> 24) as u8,
    ]
}

pub fn wavei8write(path: String, sps: u32, buf: &Vec<f32>) {
    use std::fs::File;
    use std::io::Write;
    
    let datatotalsize = buf.len() as u32 * 4;
    
    let mut fd = File::create(path).unwrap();
    fd.write("RIFF".as_bytes());                                                    // 4
    fd.write(&u32tou8ale((datatotalsize + 44) - 8)); // filesize - 8                // 4
    fd.write("WAVE".as_bytes());       //                                           // 4
    fd.write("fmt ".as_bytes());       // <format marker>                           // 4
    fd.write(&u32tou8ale(16));         // <format data length>                      // 4
    fd.write(&u16tou8ale(3));          // PCM                                       // 2
    fd.write(&u16tou8ale(1));          // 1 channel                                 // 2
    fd.write(&u32tou8ale(sps));        // sample frequency/rate                     // 4
    fd.write(&u32tou8ale(sps * 4));    // sps * bitsize * channels / 8 (byte rate)  // 4
    fd.write(&u16tou8ale(4));          // bitsize * channels / 8  (block-align)     // 2
    fd.write(&u16tou8ale(32));         // bits per sample                           // 2
    fd.write("data".as_bytes());       // <data marker>                             // 4
    fd.write(&u32tou8ale(datatotalsize));  // datasize = filesize - 44              // 4

    for x in 0..buf.len() {
        fd.write_f32::<LittleEndian>(buf[x]);
    }
    
    //unsafe { 
    //    let tmp = mem::transmute::<&Vec<i8>, &Vec<u8>>(buf);    
    //    fd.write(tmp.as_slice());
    //}
}

pub struct FileSource {
    fp:         File,    
}

impl FileSource {
    pub fn new(path: String) -> FileSource {
        FileSource {
            fp:     File::open(path).unwrap(),
        }
    }
    
    pub fn recv(&mut self) -> Vec<Complex<f32>> {
        let mut out: Vec<Complex<f32>> = Vec::new();
        for x in 0..1024*1024*20 {    
            let i = match self.fp.read_f32::<LittleEndian>() {
                Result::Ok(v) => v,
                Result::Err(_) => break,
            };
            let q = match self.fp.read_f32::<LittleEndian>() {
                Result::Ok(v) => v,
                Result::Err(_) => break,
            };
            out.push(Complex { i: i, q: q });
        }
        
        out
    }
}
