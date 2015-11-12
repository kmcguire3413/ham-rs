mod sys;

use ::libc;
use std::ffi::CString;
use std::sync::{Arc, Mutex};
use ::dsp::Complex;
use ::std;
use ::alloc;

pub struct USRPSource {
    usrp_handle:        sys::uhd_usrp_handle,
    streamer_handle:    sys::uhd_rx_streamer_handle,
    metadata_handle:    sys::uhd_rx_metadata_handle,
    buff:               *mut Complex<f32>,
    max_num_samps:      sys::size_t,
    tunereq:            sys::uhd_tune_request_t,
    streamargs:         sys::uhd_stream_args_t,
    streamcmd:          sys::uhd_stream_cmd_t,
    channel:            u64,
    buffs_ptr:          *mut *mut Complex<f32>,
}

impl USRPSource {
    pub fn new(sps: f64, center: f64, gain: f64) -> Arc<Mutex<USRPSource>> {
        let mut ausrp;
        unsafe {  
            let mut err: libc::c_uint = 0;
            
            ausrp = Arc::new(Mutex::new(USRPSource {
                usrp_handle:        std::mem::zeroed(),
                streamer_handle:    std::mem::zeroed(),
                metadata_handle:    std::mem::zeroed(),  
                buff:               0 as *mut Complex<f32>,
                buffs_ptr:          0 as *mut *mut Complex<f32>,
                max_num_samps:      0,
                tunereq:            sys::uhd_tune_request_t {
                    target_freq:        center,
                    rf_freq_policy:     sys::UHD_TUNE_REQUEST_POLICY_AUTO,
                    rf_freq:            0.0,
                    dsp_freq_policy:    sys::UHD_TUNE_REQUEST_POLICY_AUTO,
                    dsp_freq:           0.0,
                    args:               0 as *mut i8,
                },
                streamargs:         sys::uhd_stream_args_t {
                    cpu_format:         CString::new("fc32").unwrap().into_raw(), 
                    otw_format:         CString::new("sc16").unwrap().into_raw(),
                    args:               CString::new("").unwrap().into_raw(),
                    channel_list:       0 as *mut u64,
                    n_channels:         1,  
                },
                streamcmd:          sys::uhd_stream_cmd_t {
                    //stream_mode:         sys::UHD_STREAM_MODE_NUM_SAMPS_AND_DONE,
                    stream_mode:         sys::UHD_STREAM_MODE_START_CONTINUOUS,
                    num_samps:           0, // set later
                    stream_now:          1,
                    time_spec_full_secs: 0,
                    time_spec_frac_secs: 0.0,
                },
                channel:            0,
            }));
            
            let mut usrp = ausrp.lock().unwrap();
            
            usrp.streamargs.channel_list = &mut usrp.channel as *mut u64;
            
            err += sys::uhd_usrp_make(&mut usrp.usrp_handle, CString::new("").unwrap().as_ptr());
            err += sys::uhd_rx_streamer_make(&mut usrp.streamer_handle);
            err += sys::uhd_rx_metadata_make(&mut usrp.metadata_handle);
            err += sys::uhd_usrp_set_rx_rate(usrp.usrp_handle, sps, usrp.channel);
            
            let mut actual_rx_rate: f64 = 0.0;
            
            err += sys::uhd_usrp_get_rx_rate(usrp.usrp_handle, usrp.channel, &mut actual_rx_rate); 
            err += sys::uhd_usrp_set_rx_gain(usrp.usrp_handle, gain, usrp.channel, CString::new("").unwrap().as_ptr());
            
            //     pub fn uhd_usrp_set_rx_freq(h: uhd_usrp_handle,
            //                        tune_request: *mut uhd_tune_request_t,
            //                        chan: size_t,
            //                        tune_result: *mut uhd_tune_result_t)
            // -> uhd_error;
            
            let mut tuneresult: sys::uhd_tune_result_t = std::mem::zeroed();
            
            err += sys::uhd_usrp_set_rx_freq(
                usrp.usrp_handle, 
                &mut usrp.tunereq as *mut sys::uhd_tune_request_t,
                usrp.channel,
                &mut tuneresult as *mut sys::uhd_tune_result_t
            );
            
            //     pub fn uhd_usrp_get_rx_stream(h: uhd_usrp_handle,
            //      stream_args: *mut uhd_stream_args_t,
            //      h_out: uhd_rx_streamer_handle) -> uhd_error;
            
            err += sys::uhd_usrp_get_rx_stream(
                usrp.usrp_handle, 
                &mut usrp.streamargs as &mut sys::uhd_stream_args_t,
                usrp.streamer_handle
            );
            
            //     pub fn uhd_rx_streamer_max_num_samps(h: uhd_rx_streamer_handle,
            //              max_num_samps_out: *mut size_t)
            
            err += sys::uhd_rx_streamer_max_num_samps(usrp.streamer_handle, &mut usrp.max_num_samps as *mut sys::size_t);

            usrp.streamcmd.num_samps = usrp.max_num_samps;    
    
            println!("max_num_samps: {}", usrp.max_num_samps);        
            
            //     pub fn uhd_rx_streamer_issue_stream_cmd(h: uhd_rx_streamer_handle,
            //              stream_cmd:
            //              *const sys::uhd_stream_cmd_t)
            
            err += sys::uhd_rx_streamer_issue_stream_cmd(
                usrp.streamer_handle, &usrp.streamcmd as *const sys::uhd_stream_cmd_t
            );
            
            usrp.buff = alloc::heap::allocate((usrp.max_num_samps * 2 * 4) as usize, 8) as *mut Complex<f32>;
            usrp.buffs_ptr = &mut usrp.buff;
            
            // pub fn uhd_rx_streamer_recv(h: uhd_rx_streamer_handle,
            //                            buffs: *mut *mut ::libc::c_void,
            //                            samps_per_buff: size_t,
            //                            md: *mut uhd_rx_metadata_handle,
            //                            timeout: ::libc::c_double, one_packet: u8,
            //                            items_recvd: *mut size_t) -> uhd_error;        
            //
            
            println!("returning error:{}", err);         
        }    
        
        ausrp
    }
    
    pub fn recv(&mut self) -> Vec<Complex<f32>> {
        unsafe {
            let mut err: libc::c_uint;
            
            //println!("usrp trying recv");  
            
            let mut num_rx_samps: sys::size_t = 0;
            
            err = sys::uhd_rx_streamer_recv(
                self.streamer_handle,
                self.buffs_ptr as *mut *mut libc::c_void,
                self.max_num_samps,
                &mut self.metadata_handle,
                3.0,
                0,
                &mut num_rx_samps as *mut sys::size_t
            );
            
            //println!("recv err {}", err);

            // Treat our buffer as a vector of Complex<f32> items.
            let sbuf: Vec<Complex<f32>> = Vec::from_raw_parts(self.buff, num_rx_samps as usize, num_rx_samps as usize);                  
            
            //println!("returning {} samples ... {} sizeof:{} alignof:{}", sbuf.len(), num_rx_samps, std::mem::size_of::<Complex<f32>>(),
            //    std::mem::align_of::<Complex<f32>>()
            //);
            
            // Copy the Vec (array).
            let sbuf_clone = sbuf.clone();
            
            // If we do not forget it then it appears that Vec decides
            // to deallocate on its own accord, thus, we shall crash. This
            // is the most safe manner in which we can operate for potential
            // future functionality changes to Vec.
            std::mem::forget(sbuf);
            
            // This is the new memory buffer that has been allocated.
            sbuf_clone
        }
    }
}
