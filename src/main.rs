#![feature(path_ext)]
#![feature(convert)]
#![feature(deque_extras)]
extern crate lodepng;
extern crate byteorder;
extern crate alsa;
extern crate num;

use std::cmp::Ordering;
use std::io::{SeekFrom, Seek, Cursor, Read};
use std::path::{Path};
use std::fs::{File, PathExt};
use byteorder::{ReadBytesExt, WriteBytesExt, LittleEndian};
use std::mem;
use std::str::FromStr;
use std::thread;

mod algos;
mod dsp;

use algos::SignalMap;
use algos::mcguire_smde;

use dsp::Complex;

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

fn wavei8write(path: String, sps: u32, buf: &Vec<f32>) {
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

fn buildsine(freq: f64, sps: f64, amp: f32) -> Option<Vec<Complex<f32>>> {
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


struct FMDemod {
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
}

impl FMDemod {
    pub fn new(sps: f64, offset: f64, bw: f32, taps: Vec<f32>) -> FMDemod {
        let ifs = buildsine(-240000.0, 4000000.0, 5.0).unwrap();
        
        let mut tapvi: Vec<f32> = Vec::new();
        let mut tapvq: Vec<f32> = Vec::new();
        
        for x in 0..taps.len() {
            tapvi.push(0.0);
            tapvq.push(0.0);
        }
            
    	FMDemod { 
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
    	}
    }
    
    pub fn work(&mut self, stream: &Vec<Complex<f32>>) -> Vec<f32> {
        let mut buf: Vec<f32> = Vec::with_capacity(stream.len() / 5 / 50); 
        
        let fmaxphaserot = ((std::f32::consts::PI * 2.0) / (4000000.0 / 5.0)) * self.bw;   
    
        for x in 0..stream.len() {
            let mut s = stream[x].clone();            
                        
            s.mul(&self.ifs[self.ifsndx]);
            
            self.ifsndx = if self.ifsndx + 1 >= self.ifs.len() {
                0
            } else {
                self.ifsndx + 1
            };   

            if self.q0 == 5 {
                self.q0 = 0;
                
                // tapvi[tapi]  19
                // tapvq[tapi]  19
                // taps
                
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
                        
                //let pwr = (s.i * s.i + s.q * s.q).sqrt();

                let mut a = s.i.atan2(s.q);
                let mut b = self.li.atan2(self.lq);
                
                if a < 0.0 { a = std::f32::consts::PI + a + std::f32::consts::PI; }
                if b < 0.0 { b = std::f32::consts::PI + b + std::f32::consts::PI; }        
                
                let mut r = 0f32;
                
                r = a - b;
                
                if r > std::f32::consts::PI {
                    r = std::f32::consts::PI * 2.0 + r;
                }                
                
                //if b > a {
                //    if b - a > std::f32::consts::PI {
                //        r = ((a + std::f32::consts::PI * 2.0) - b);                       
                //    } else {
                //        r = (a - b);
                //    }
                //} else {
                //    r = (a - b);
                //}     
                
                // This limits sharp impulses where spikes have slipped
                // through our taps filter.
                if r.abs() < fmaxphaserot {
                    self.rsum += r;
                }  
                
                self.q1 += 1; 
                if self.q1 == 50 {
                    self.q1 = 0;
                    
                    self.rsum /= 50f32;
                                        
                    buf.push(self.rsum);
                    
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

fn main() {
    /*
    let fpath: &Path = Path::new("/home/kmcguire/Projects/radiowork/usbstore/test147.840");
    
    let fsize: usize = match fpath.metadata() {
        Ok(md) => md.len() as usize,
        Err(_) => panic!("Could not get file length."),
    };
    
    let mut fp = File::open(fpath).unwrap();

    let mut min = std::f64::MAX;
    let mut max = std::f64::MIN;
    let mut sum = 0f64;

    for x in 0..fsize / 4 {
        let sample: f64 = fp.read_f32::<LittleEndian>().unwrap() as f64;
        
        if sample < min {
            min = sample;
        }
        
        if sample > max {
            max = sample;
        }
        
        sum += sample;
    }
    
    println!("min:{} max:{} avg:{}", min, max, sum / ((fsize as f64) / 4f64));
    
    if true { return; }    
    */
    
    //other();

    //if true {
    //    return;
    //}

    let fpath: &Path = Path::new("/home/kmcguire/Projects/radiowork/usbstore/recording01");
    
    let fsize: usize = match fpath.metadata() {
        Ok(md) => md.len() as usize,
        Err(_) => panic!("Could not get file length."),
    };
    
    let mut fp = File::open(fpath).unwrap();    

    fp.seek(SeekFrom::Start(0)).unwrap();

    let mut q0 = 0; 
    let mut q1 = 0;
    let mut fmlast_i = 0f32;
    let mut fmlast_q = 0f32;
    
    let mut buf: Vec<f32> = Vec::new();
    
    let bufsubsize = 1024 * 1024 * 290;     

    let mut fbuf: Vec<u8> = Vec::with_capacity(bufsubsize);
    
    unsafe {
        fbuf.set_len(bufsubsize);
    }       
    
    let mut rsum = 0f32;
    
    let mut lr = 0f32;
    
    let mut pwr_min = std::f32::MAX;
    let mut pwr_max = std::f32::MIN;
    
    //fn buildsine(freq: f32, sps: f32, amp: f32) -> Option<Vec<Complex<f32>>> {

    
    let taps: Vec<f32> = vec![0.39760953187942505f32, 0.22784264385700226, -1.549405637309571e-16, -0.25822165608406067, -0.5112122297286987, -0.7193102836608887, -0.8434571623802185, -0.8500939607620239, -0.7156971096992493, -0.43036943674087524, 1.549405637309571e-16, 0.5533321499824524, 1.19282865524292, 1.8702067136764526, 2.5303714275360107, 3.117011308670044, 3.5784854888916016, 3.8733248710632324, 3.974698305130005, 3.8733248710632324, 3.5784854888916016, 3.117011308670044, 2.5303714275360107, 1.8702067136764526, 1.19282865524292, 0.5533321499824524, 1.549405637309571e-16, -0.43036943674087524, -0.7156971096992493, -0.8500939607620239, -0.8434571623802185, -0.7193102836608887, -0.5112122297286987, -0.25822165608406067, -1.549405637309571e-16, 0.22784264385700226, 0.39760953187942505];
    
    let mut fmdemod = FMDemod::new(4000000.0, -240000.0, 30000.0, taps);

    println!("processing");
    
    let mut buf: Vec<f32> = Vec::new();
    let mut ibuf: Vec<Complex<f32>> = Vec::with_capacity(fbuf.len() / 8);    
    
    for x in 0..fsize / bufsubsize {
        let fbuflen = fp.read(&mut fbuf).unwrap();
        println!("fbuflen:{}", fbuflen);
        let mut rdr = Cursor::new(fbuf);

        ibuf.clear();
        loop {
            let i = match rdr.read_f32::<LittleEndian>() {
                Result::Err(_) => break,
                Result::Ok(v) => v,       
            };
            let q = match rdr.read_f32::<LittleEndian>() {
                Result::Err(_) => break,
                Result::Ok(v) => v,
            };
            ibuf.push(Complex { i: i, q: q });
        }
        println!("decoding block");
        let mut out = fmdemod.work(&ibuf);
        buf.append(&mut out);            
        println!("wavbuf:{}", buf.len());
        fbuf = rdr.into_inner();
    } 
    
    wavei8write(String::from_str("tmp.wav").unwrap(), 16000, &buf);
    
    println!("done");
}


fn other() {
    let fpath: &Path = Path::new("/home/kmcguire/Projects/radiowork/usbstore/spectrum");
    
    let fsize: usize = match fpath.metadata() {
        Ok(md) => md.len() as usize,
        Err(_) => panic!("Could not get file length."),
    };
    
    let mut fp = File::open(fpath).unwrap();
    
    let fw: usize = 1024;
    let fh: usize = fsize / 4 / fw;

    let factorw: usize = 1;
    let factorh: usize = 4;    
    
    let aw: usize = fw / factorw;
    let ah: usize = fh / factorh;
    
    let mut abuffer: Vec<f64> = Vec::with_capacity(aw * ah * 2); 
    let mut pbuffer: Vec<u8> = Vec::with_capacity(aw * ah * 4);
    
    for _ in 0..aw * ah {
        abuffer.push(0.0);
        abuffer.push(0.0);
        pbuffer.push(0);
        pbuffer.push(0);
        pbuffer.push(0);
        pbuffer.push(0);
    }
    
    let mut sample_max: f64 = std::f64::MIN;
    let mut sample_min: f64 = std::f64::MAX;
    
    for col in 0..ah {
        println!("working col: {}/{}", col, ah); 
        for row in 0..aw {
            let rowrng = (row * factorw, row * factorw + factorw);
            let colrng = (col * factorh, col * factorh + factorh);
            for fcol in (colrng.0)..(colrng.1) {
                fp.seek(SeekFrom::Start((fcol * fw + rowrng.0) as u64 * 4)).unwrap();
                for _ in 0..factorw {
                    let sample: f64 = fp.read_f32::<LittleEndian>().unwrap() as f64;      
                    abuffer[(row + col * aw) * 2 + 0] += sample as f64;
                    abuffer[(row + col * aw) * 2 + 1] += 1.0f64;
                }
            }
        }        
    }
    
    /*
    let mut _tmp: Vec<f64> = Vec::with_capacity(aw*ah);
    
    // Produce the average per each output unit.
    for row in 0..ah {
        for col in 0..aw {
            _tmp.push(abuffer[(row * aw + col) * 2 + 0] / abuffer[(row * aw + col) * 2 + 1]);
        }
    }    

    let mut amap = SignalMap {
        v:      _tmp,
        w:      aw,
        h:      ah,
    };    
     
    for _ in 0..1 {
        amap = mcguire_smde::multi_all(&amap);
        amap.normalize();
    }
    
    let mut __tmp: Vec<u8> = Vec::with_capacity(amap.w * amap.h);
    
    for x in 0..amap.v.len() {
        __tmp.push((amap.v[x] * 255f64) as u8); // R
        __tmp.push((amap.v[x] * 255f64) as u8); // G
        __tmp.push((amap.v[x] * 255f64) as u8); // B
        __tmp.push(255);
    }
    
    lodepng::encode_file(
        "ana.png", __tmp.as_slice(),
        amap.w, amap.h,
        lodepng::ColorType::LCT_RGBA, 8
    ).unwrap();    
    */
    
    let space = (3.1415 * 2.0 / 3.0) as f64;
    
    let mut rowdata: Vec<f64> = Vec::with_capacity(aw);
    
    for i in 0..aw {
        rowdata.push(0.0f64);
    }    

    let mut gmin: f64 = std::f64::MAX;
    let mut gmax: f64 = std::f64::MIN;
    
    println!("gmin:{} gmax:{}", gmin, gmax);  
    
    for i in 0..aw * ah {
        let f = abuffer[i * 2 + 0] / abuffer[i * 2 + 1];
        if f < gmin {
            gmin = f;
        }
        
        if f > gmax {
            gmax = f;
        }
    }
    
    for col in 0..ah {
        let colpos = col * aw;
        
        let mut min: f64 = std::f64::MAX;
        let mut max: f64 = std::f64::MIN;
        
        for row in 0..aw {
            rowdata[row] = abuffer[(colpos + row) * 2 + 0] / abuffer[(colpos + row) * 2 + 1];
            
            if rowdata[row] > max {
                max = rowdata[row];
            }
            
            if rowdata[row] < min {
                min = rowdata[row];
            }
        }
            
        for row in 0..aw {
            // Large differences get a dark value so invert the normalization.
            let mut samplenorm = 1.0 - (rowdata[row] - gmin) / (gmax - gmin);
            
            //let i: f64 = samplenorm * 3.1415 * 2.0;
            
            //let r = (i.sin() * 255f64) as u8;
            //let g = ((i + space).sin() * 255f64) as u8;
            //let b = ((i + space * 2f64).sin() * 255f64) as u8;
            
            let r = samplenorm * 255f64;
            let g = samplenorm * 255f64;
            let b = samplenorm * 255f64; 
            
            pbuffer[(colpos + row) * 4 + 0] = r as u8;
            pbuffer[(colpos + row) * 4 + 1] = g as u8;
            pbuffer[(colpos + row) * 4 + 2] = b as u8;
            pbuffer[(colpos + row) * 4 + 3] = 255 as u8;
        }
    }    

    println!("aw:{} ah:{} pbuffer.len():{}", aw, ah, pbuffer.len());
    lodepng::encode_file(
        "out.png", pbuffer.as_slice(),
        aw, ah,
        lodepng::ColorType::LCT_RGBA, 8
    ).unwrap();
}
