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
use std::ffi::CString;

mod usrp;
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
    decim:      usize,
    curslack:   f32,
    maxphase:   f32,
    slack:      f32,
    audiodecim: usize,
}

impl FMDemod {
    pub fn new(sps: f64, decim: usize, offset: f64, bw: f32, taps: Vec<f32>) -> FMDemod {
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

struct USRPSource {
    usrp_handle:        usrp::uhd_usrp_handle,
    streamer_handle:    usrp::uhd_rx_streamer_handle,
    metadata_handle:    usrp::uhd_rx_metadata_handle,
    buff:               *mut Complex<f32>,
    buffs_ptr:          *mut *mut Complex<f32>,
    max_num_samps:      usrp::size_t,
    
}

impl USRPSource {
    pub fn new() -> USRPSource {
    let mut usrp_channel: u64 = 0;
        unsafe {  
        
            let mut usrp_handle: usrp::uhd_usrp_handle = std::mem::zeroed();
            let mut streamer_handle: usrp::uhd_rx_streamer_handle = std::mem::zeroed();
            let mut metadata_handle: usrp::uhd_rx_metadata_handle = std::mem::zeroed();
            let mut tunereq = usrp::uhd_tune_request_t {
                target_freq:        146000000.0,
                rf_freq_policy:     usrp::UHD_TUNE_REQUEST_POLICY_AUTO,
                rf_freq:            0.0,
                dsp_freq_policy:    usrp::UHD_TUNE_REQUEST_POLICY_AUTO,
                dsp_freq:           0.0,
                args:               0 as *mut i8,
            };
            let mut streamargs = usrp::uhd_stream_args_t {
                cpu_format:         CString::new("fc32").unwrap().into_raw(), 
                otw_format:         CString::new("sc16").unwrap().into_raw(),
                args:               CString::new("").unwrap().into_raw(),
                channel_list:       &mut usrp_channel as *mut u64,
                n_channels:         1,  
            };
            
            let mut streamcmd = usrp::uhd_stream_cmd_t {
                //stream_mode:         usrp::UHD_STREAM_MODE_NUM_SAMPS_AND_DONE,
                stream_mode:         usrp::UHD_STREAM_MODE_START_CONTINUOUS,
                num_samps:           0,
                stream_now:          1,
                time_spec_full_secs: 0,
                time_spec_frac_secs: 0.0,
            };
            
            usrp::uhd_usrp_make(&mut usrp_handle, CString::new("").unwrap().as_ptr());
            usrp::uhd_rx_streamer_make(&mut streamer_handle);
            usrp::uhd_rx_metadata_make(&mut metadata_handle);
            usrp::uhd_usrp_set_rx_rate(usrp_handle, 800000.0, usrp_channel);
            let mut actual_rx_rate: f64 = 0.0;
            usrp::uhd_usrp_get_rx_rate(usrp_handle, usrp_channel, &mut actual_rx_rate); 
            usrp::uhd_usrp_set_rx_gain(usrp_handle, 20.0, usrp_channel, CString::new("").unwrap().as_ptr());
            
            //     pub fn uhd_usrp_set_rx_freq(h: uhd_usrp_handle,
            //                        tune_request: *mut uhd_tune_request_t,
            //                        chan: size_t,
            //                        tune_result: *mut uhd_tune_result_t)
            // -> uhd_error;
            
            let mut tuneresult: usrp::uhd_tune_result_t = std::mem::zeroed();
            
            usrp::uhd_usrp_set_rx_freq(
                usrp_handle, 
                &mut tunereq as *mut usrp::uhd_tune_request_t,
                usrp_channel,
                &mut tuneresult as *mut usrp::uhd_tune_result_t
            );
            
            //     pub fn uhd_usrp_get_rx_stream(h: uhd_usrp_handle,
            //      stream_args: *mut uhd_stream_args_t,
            //      h_out: uhd_rx_streamer_handle) -> uhd_error;
            
            usrp::uhd_usrp_get_rx_stream(
                usrp_handle, 
                &mut streamargs as &mut usrp::uhd_stream_args_t,
                streamer_handle
            );
            
            //     pub fn uhd_rx_streamer_max_num_samps(h: uhd_rx_streamer_handle,
            //              max_num_samps_out: *mut size_t)
            
            let mut max_num_samps: usrp::size_t = 0;
            
            usrp::uhd_rx_streamer_max_num_samps(streamer_handle, &mut max_num_samps as *mut usrp::size_t);
    
            println!("max_num_samps: {}", max_num_samps);        
            
            //     pub fn uhd_rx_streamer_issue_stream_cmd(h: uhd_rx_streamer_handle,
            //              stream_cmd:
            //              *const usrp::uhd_stream_cmd_t)
            
            usrp::uhd_rx_streamer_issue_stream_cmd(
                streamer_handle, &streamcmd as *const usrp::uhd_stream_cmd_t
            );
            
            let mut buff: *mut Complex<f32> = alloc::heap::allocate((max_num_samps * 2 * 4) as usize, 4) as *mut Complex<f32>;
            let mut buffs_ptr: *mut *mut Complex<f32> = &mut buff;
            
            // pub fn uhd_rx_streamer_recv(h: uhd_rx_streamer_handle,
            //                            buffs: *mut *mut ::libc::c_void,
            //                            samps_per_buff: size_t,
            //                            md: *mut uhd_rx_metadata_handle,
            //                            timeout: ::libc::c_double, one_packet: u8,
            //                            items_recvd: *mut size_t) -> uhd_error;        
            //
            
            USRPSource {
                usrp_handle:        usrp_handle,
                streamer_handle:    streamer_handle,
                metadata_handle:    metadata_handle,  
                buff:               buff,
                buffs_ptr:          buffs_ptr,    
                max_num_samps:      max_num_samps,                          
            }                        
        }    
    }
    
    pub fn recv(&mut self) {
        unsafe {
            let mut err;
            
            println!("usrp trying recv");  
            
            let mut num_rx_samps: usrp::size_t = 0;             
            
            err = usrp::uhd_rx_streamer_recv(
                self.streamer_handle,
                self.buffs_ptr as *mut *mut libc::c_void,
                self.max_num_samps,
                &mut self.metadata_handle,
                3.0,
                0,
                &mut num_rx_samps as *mut usrp::size_t
            );
    
            let sbuf: Vec<Complex<f32>> = Vec::from_raw_parts(self.buff, self.max_num_samps as usize * 8, self.max_num_samps as usize * 8);         
            
            for x in 0..sbuf.len() {
                let i = sbuf[x].i;
                let q = sbuf[x].q;
                if i == 0.0 && q == 0.0 {
                    break;
                }
                println!("x:{} vector: {},{}", x, i, q);
            }
            
            println!("num_samples: {}", num_rx_samps);
            
            println!("usrp done streaming {}", err);
        }
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

    // pub fn uhd_usrp_make(h: *mut uhd_usrp_handle, args: *const ::libc::c_char) -> uhd_error;
    // pub fn uhd_rx_streamer_make(h: *mut uhd_rx_streamer_handle) -> uhd_error;
    // pub fn uhd_rx_metadata_make(handle: *mut uhd_rx_metadata_handle) -> uhd_error;
    
    if true {
        return;
    }
    
    let taps: Vec<f32> = vec![0.44378024339675903f32, 0.9566655158996582, 1.4999324083328247, 2.0293939113616943, 2.499887466430664, 2.8699963092803955, 3.106461763381958, 3.1877646446228027, 3.106461763381958, 2.8699963092803955, 2.499887466430664, 2.0293939113616943, 1.4999324083328247, 0.9566655158996582, 0.44378024339675903];
    
    let mut fmdemod = FMDemod::new(4000000.0, 11, -840000.0, 15000.0, taps);

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
        let st = time::precise_time_ns();
        let mut out = fmdemod.work(&ibuf);
        println!("done {}", (time::precise_time_ns() - st) as f64 / 1000.0 / 1000.0 / 1000.0);
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
