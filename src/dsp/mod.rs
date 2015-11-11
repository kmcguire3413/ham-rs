use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Sender, Receiver, SendError, TryRecvError, channel};
use std::mem;

use std::path::{Path};
use std::fs::{File, PathExt};
use byteorder::{WriteBytesExt, LittleEndian};

use std::ffi::CString;
use alsa::{Direction, ValueOr};
use alsa::pcm::{PCM, HwParams, Format, Access, State};

use std::collections::VecDeque;

pub enum DSPBlockAddResult {
    Success,
    Failure,
}

pub enum DSPChannelResult<T> {
    BrokenChannel,
    Failure,
    Success(T),
}

pub struct DSPSender<T> {
    core:       Arc<DSPChannelCore<T>>,
}

pub struct DSPReceiver<T> {
    core:       Arc<DSPChannelCore<T>>,
}

#[derive(Clone)]
pub struct Complex<T> {
    pub i:              T,
    pub q:              T,
}

pub struct DSPChannelCore<T> {
    buffer:     Mutex<VecDeque<T>>,
}

pub fn dspchan_create<T>() -> (DSPSender<T>, DSPReceiver<T>) {
    let core = Arc::new(DSPChannelCore {
        buffer: Mutex::new(VecDeque::<T>::new())
    });
    
    (
        DSPSender::new(core.clone()),
        DSPReceiver::new(core)
    )
}

impl<T> DSPSender<T> {
    pub fn new(core: Arc<DSPChannelCore<T>>) -> DSPSender<T> {
        DSPSender { core: core }
    }
    pub fn send(&self, v: T) -> DSPChannelResult<()> {
        let mut buffer = self.core.buffer.lock().unwrap();
        buffer.push_back(v);
        DSPChannelResult::Success(())
    }
}

impl<T> DSPReceiver<T> {
    pub fn new(core: Arc<DSPChannelCore<T>>) -> DSPReceiver<T> {
        DSPReceiver { 
            core:   core, 
        }
    }
    
    fn dump(&mut self) {
        let mut buffer = self.core.buffer.lock().unwrap();
        buffer.truncate(0);
    }
    
    fn recv(&self) -> DSPChannelResult<T> {
        let mut buffer = self.core.buffer.lock().unwrap();
        match buffer.pop_front() {
            Option::Some(v) => DSPChannelResult::Success(v),
            Option::None => DSPChannelResult::Failure,
        }
    }
}

pub struct DSPBlockSplitter<T> {
    rx:     Option<DSPReceiver<T>>,
    tx:     Vec<DSPSender<T>>,
}

impl<T: Clone> DSPBlockSplitter<T> {
    pub fn new() -> DSPBlockSplitter<T> {
        DSPBlockSplitter { rx: Option::None, tx: Vec::new() }
    }
    
    pub fn set_input(&mut self, rx: DSPReceiver<T>) {
        self.rx = Option::Some(rx);
    }
    
    pub fn add_output(&mut self, tx: DSPSender<T>) {
        self.tx.push(tx);
    }
    
    pub fn tick_uoi(&mut self) {
        let mut _rx: Option<DSPReceiver<T>> = Option::None;
        
        mem::swap(&mut _rx, &mut self.rx);    
    
        match _rx {
            Option::None => (),
            Option::Some(ref mut rx) => {
                loop {
                    match rx.recv() {
                        DSPChannelResult::BrokenChannel => break,
                        DSPChannelResult::Failure => break,
                        DSPChannelResult::Success(v) => {
                            for x in 0..self.tx.len() {
                                self.tx[x].send(v.clone());                 
                            }
                        },                        
                    }
                }                
            }
        }
        
        mem::swap(&mut _rx, &mut self.rx);
    }
}

pub struct DSPBlockLowPass {
    tx:     Option<DSPSender<Complex<f64>>>,
    rx:     Option<DSPReceiver<Complex<f64>>>,  
    last:   f64,
    factor: f64,  
}

pub struct DSPBlockFMDecode {
    tx:     Option<DSPSender<f64>>,
    rx:     Option<DSPReceiver<Complex<f64>>>,
    last:   Complex<f64>, 
}

impl DSPBlockFMDecode {
    pub fn new() -> DSPBlockFMDecode {
        DSPBlockFMDecode { tx: Option::None, rx: Option::None, last: Complex { i: 0.0, q: 0.0 } }
    }
    
    pub fn set_input(&mut self, rx: DSPReceiver<Complex<f64>>) {
        self.rx = Option::Some(rx);
    }
    
    pub fn set_output(&mut self, tx: DSPSender<f64>) {
        self.tx = Option::Some(tx);
    }
    
    fn _tick_uoi(&mut self, rx: &mut DSPReceiver<Complex<f64>>, tx: &mut DSPSender<f64>) {
        loop {
            match rx.recv() {
            	DSPChannelResult::BrokenChannel => break,
            	DSPChannelResult::Failure => break,
            	DSPChannelResult::Success(v) => {
            	   let a = v.i / v.q;
            	   let b = self.last.i / self.last.q;
            	   let c = (b - a).atan();
            	   let r = c / 3.1415;
            	   self.last = v;
            	   tx.send(r);
            	}, 
            }
        }
    } 
    
    pub fn tick_uoi(&mut self) {
        let mut _tx: Option<DSPSender<f64>> = Option::None;
        let mut _rx: Option<DSPReceiver<Complex<f64>>> = Option::None;
        
        mem::swap(&mut self.tx, &mut _tx);
        mem::swap(&mut self.rx, &mut _rx);    

        match _rx {
            Option::None => (),
            Option::Some(ref mut rx) => match _tx {
                Option::None => (),
                Option::Some(ref mut tx) => {
                    self._tick_uoi(rx, tx);
                }
            }
        }
        
        mem::swap(&mut self.tx, &mut _tx);
        mem::swap(&mut self.rx, &mut _rx);            
    }
}

pub struct DSPBlockSinkNull {
    rx:     Option<DSPReceiver<Complex<f64>>>,
}

impl DSPBlockSinkNull {
    pub fn new() -> DSPBlockSinkNull {
        DSPBlockSinkNull { rx: Option::None }
    }

    pub fn set_input(&mut self, rx: DSPReceiver<Complex<f64>>) -> DSPBlockAddResult {
        self.rx = Option::Some(rx);
        DSPBlockAddResult::Success    
    }
    pub fn set_output(&mut self, rx: DSPSender<Complex<f64>>) -> DSPBlockAddResult {
        DSPBlockAddResult::Failure
    }
    pub fn tick_uoi(&mut self) {
        match self.rx {
            Option::None => return,
            Option::Some(ref mut rx) => rx.dump(),
        }    
    }
}

impl DSPBlockLowPass {
    pub fn new(factor: f64) -> DSPBlockLowPass {
        DSPBlockLowPass {
            tx:       Option::None,
            rx:       Option::None,
            last:     0f64,
            factor:   factor,
        }
    }
    
    pub fn set_output(&mut self, tx: DSPSender<Complex<f64>>) -> DSPBlockAddResult {
        self.tx = Option::Some(tx);
        DSPBlockAddResult::Success
    }
    /// Add an input channel to this block.
    ///
    /// _Adding more than one will fail and return a failure code._
    pub fn set_input(&mut self, rx: DSPReceiver<Complex<f64>>) -> DSPBlockAddResult {
        self.rx = Option::Some(rx);
        DSPBlockAddResult::Success
    }
    ///
    pub fn tick_uoi(&mut self) {
        loop {
            match self.rx {
                Option::None => return,
                Option::Some(ref mut rx) => match self.tx {
                    Option::None => return,
                    Option::Some(ref mut tx) => {
                        match rx.recv() {
                            // This should have happened because of an actual failure.
                            DSPChannelResult::BrokenChannel => return,
                            // This should have happened because there was no data.
                            DSPChannelResult::Failure => return,
                            DSPChannelResult::Success(mut v) => {
                                // Find out the change in magnitude from the last sample.
                                //let v = ary[x];
    
                                let m = (v.i * v.i + v.q * v.q).sqrt();                            
                                let delta = m - self.last;

                                v.i = v.i / (delta * self.factor);
                                v.q = v.q / (delta * self.factor);                                
                                
                                self.last = m;                                
                                
                                tx.send(v);
                            }
                        }
                    }
                }
            }
        }
    }
}

pub struct DSPBlockSinkAudio {
    sps:            u32,
    rx:             Option<DSPReceiver<i16>>,
    pcm:            PCM,
    consumed:       usize,
}

pub struct DSPBlockConvert<A, B> {
    tx:             Option<DSPSender<B>>,
    rx:             Option<DSPReceiver<A>>,
    decim:          usize,
    decimleft:      usize,
}

pub trait DSPBlockConvertSpec {
    type     InType;
    type     OutType;
    
    fn tick_uoi(&mut self);
    fn read_and_convert(&mut self, rx: &mut DSPReceiver<Self::InType>, tx: &mut DSPSender<Self::OutType>);
}

pub struct DSPBlockSinkFile<A> {
    rx:             Option<DSPReceiver<A>>,
    fp:             File,
}

impl<A> DSPBlockSinkFile<A> {
    pub fn new(path: &Path) -> DSPBlockSinkFile<A> {
        let fp = File::create(path).unwrap();
        DSPBlockSinkFile {
            rx:         Option::None,
            fp:         fp,
        }
    }
    
    pub fn set_input(&mut self, rx: DSPReceiver<A>) {
        self.rx = Option::Some(rx);
    }
}

pub trait DSPBlockSinkFileSpec {
    type     InType;
    
    fn tick_uoi(&mut self);
    fn read_and_store(&mut self, rx: &mut DSPReceiver<Self::InType>);
}

impl DSPBlockSinkFileSpec for DSPBlockSinkFile<f64> {
    type    InType = f64;
    
    fn read_and_store(&mut self, rx: &mut DSPReceiver<Self::InType>) {
        loop {
            match rx.recv() {
                DSPChannelResult::BrokenChannel => break,
                DSPChannelResult::Failure => break,
                DSPChannelResult::Success(v) => {
                    self.fp.write_f32::<LittleEndian>(v as f32).unwrap();
                },
            }
        }
    }
    
    fn tick_uoi(&mut self) {
        let mut _rx: Option<DSPReceiver<Self::InType>> = Option::None;
        
        mem::swap(&mut _rx, &mut self.rx);

        match _rx {
            Option::None => (),
            Option::Some(ref mut rx) => self.read_and_store(rx),
        }        
        
        mem::swap(&mut _rx, &mut self.rx);        
    }
}

impl DSPBlockSinkFileSpec for DSPBlockSinkFile<Complex<f64>> {
    type    InType = Complex<f64>;
    
    fn read_and_store(&mut self, rx: &mut DSPReceiver<Self::InType>) {
        loop {
            match rx.recv() {
                DSPChannelResult::BrokenChannel => break,
                DSPChannelResult::Failure => break,
                DSPChannelResult::Success(v) => {
                    self.fp.write_f32::<LittleEndian>(v.i as f32).unwrap();
                    self.fp.write_f32::<LittleEndian>(v.q as f32).unwrap();
                },
            }
        }
    }
    
    fn tick_uoi(&mut self) {
        let mut _rx: Option<DSPReceiver<Self::InType>> = Option::None;
        
        mem::swap(&mut _rx, &mut self.rx);

        match _rx {
            Option::None => (),
            Option::Some(ref mut rx) => self.read_and_store(rx),
        }        
        
        mem::swap(&mut _rx, &mut self.rx);        
    }
}

impl DSPBlockConvertSpec for DSPBlockConvert<f64, i16> {
    type    InType = f64;
    type    OutType = i16;

    fn read_and_convert(&mut self, rx: &mut DSPReceiver<Self::InType>, tx: &mut DSPSender<Self::OutType>) {
        loop {
            match rx.recv() {
                DSPChannelResult::BrokenChannel => break,
                DSPChannelResult::Failure => break,
                DSPChannelResult::Success(v) => {
                    if self.decimleft == 0 {
                        self.decimleft = self.decim;
                        tx.send((v * 2000f64) as i16);
                    }
                    self.decimleft -= 1;
                },      
            }
        }
    }
    
    fn tick_uoi(&mut self) {
        let mut _tx: Option<DSPSender<i16>> = Option::None;
        let mut _rx: Option<DSPReceiver<f64>> = Option::None;
        
        mem::swap(&mut self.tx, &mut _tx);
        mem::swap(&mut self.rx, &mut _rx);    
    
        match _rx {
            Option::None => (),
            Option::Some(ref mut rx) => match _tx {
                Option::None => (),
                Option::Some(ref mut tx) => {
                    self.read_and_convert(rx, tx);
                }
            }
        }    
        
        mem::swap(&mut self.tx, &mut _tx);
        mem::swap(&mut self.rx, &mut _rx);
    }    
}

impl<A, B> DSPBlockConvert<A, B> {
    pub fn new(decim: usize) -> DSPBlockConvert<A, B> {
        DSPBlockConvert { tx: Option::None, rx: Option::None, decim: decim, decimleft: 0 }
    }
    
    pub fn set_input(&mut self, rx: DSPReceiver<A>) {
        self.rx = Option::Some(rx);
    }
    
    pub fn set_output(&mut self, tx: DSPSender<B>) {
        self.tx = Option::Some(tx);
    }    
}

impl DSPBlockSinkAudio {
    pub fn new(sps: u32) -> DSPBlockSinkAudio {
        // Open default playback device
        let pcm = PCM::open(&*CString::new("default").unwrap(), Direction::Playback, false).unwrap();
        
        // Set hardware parameters: 44100 Hz / Mono / 16 bit
        {
            let hwp = HwParams::any(&pcm).unwrap();
            hwp.set_channels(1).unwrap();
            hwp.set_rate(44100, ValueOr::Nearest).unwrap();
            hwp.set_format(Format::s16()).unwrap();
            hwp.set_access(Access::RWInterleaved).unwrap();
            pcm.hw_params(&hwp).unwrap();
        }
            
        DSPBlockSinkAudio { sps: sps, rx: Option::None, pcm: pcm, consumed: 0 }
    }
    
    
    pub fn set_input(&mut self, rx: DSPReceiver<i16>) {
        self.rx = Option::Some(rx);
    }
    
    pub fn get_consumed(&self) -> usize {
        self.consumed
    }
    
    pub fn tick_uoi(&mut self) {
        println!("trying to write audio");
        loop {
            match self.rx {
                Option::None => return,
                Option::Some(ref mut rx) => {
                    match rx.recv() {
                        DSPChannelResult::BrokenChannel => return,
                        DSPChannelResult::Failure => return,
                        DSPChannelResult::Success(v) => {
                            let io = self.pcm.io_i16().unwrap();
                            self.consumed += 1;
                            io.writei(&[v]);
                        }                                                
                    }                    
                },                
            }
        }
        println!("done writing audio");
    }
}

/*
// Make a sine wave
let mut buf = [0i16; 1024];
for (i, a) in buf.iter_mut().enumerate() {
    *a = ((i as f32 * 2.0 * ::std::f32::consts::PI / 128.0).sin() * 8192.0) as i16
}

// Play it back for 2 seconds.
for _ in 0..2*44100/1024 {
    assert_eq!(io.writei(&buf[..]).unwrap(), 1024);
}

// In case the buffer was larger than 2 seconds, start the stream manually.
if pcm.state() != State::Running { pcm.start().unwrap() };
// Wait for the stream to finish playback.
pcm.drain().unwrap();
*/