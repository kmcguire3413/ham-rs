///! McGuire Spectral Mean Differentiated Energy
///!
///! This algorithm is used to analyze a frequency waterfall output
///! and produce output that correlates to strong and weak spectral
///! energy that has significant differences in energy levels on the
///! side bands.
///!
///! The concept for this algorithm was the emulate the human eye in
///! reading frequency waterfall plots. The human eye is sensitive to
///! detecting structure in such as average intensity along the time
///! axis and most especially bordering signals which may be mostly
///! noise. 
///!
///! The algorithm does not do particularly well in determining structure
///! in the signal, but instead relies on a more naive technique of
///! averaging energy changes, however, even this technique can produce
///! output that shows signals that might have been difficult to see with
///! the human eye.

use super::SignalMap;
use std;

///! Like the `mcguire_smde_single`, but the _single_ is called for varying
///! sized blocks of the input all at powers of two. This produces output
///! that has sampled the input in various sized blocks.
pub fn multi_all_bucketed(smap: &SignalMap) -> Vec<Vec<f64>> {
    let mut out: Vec<Vec<f64>> = Vec::new();
    let mut p = 1u32;
    
    while (2usize.pow(p) < smap.h as usize) {
        let bsize = 2usize.pow(p);
        let bcount = smap.h / bsize;
        for bcur in 0..bcount {
            let row = single(smap, bcur * bsize, bcur * bsize + bsize);
            out.push(row);
        }
        p += 1;
    }
    
    out
}

///! Produce a signal map using the output from `mcguire_multi_all_bucketed`.
///!
///! _This is useful for the effect of reapplying the algorithm on the output
///! as many times as desired._
pub fn multi_all(smap: &SignalMap) -> SignalMap {
    let out = multi_all_bucketed(smap);
    let mut v: Vec<f64> = Vec::new();
    for x in 0..out.len() {
        for y in 0..out[x].len() {
            v.push(out[x][y]);
        }
    }
    let nsmap = SignalMap {
        v:      v,
        w:      smap.w,
        h:      out.len(),
    };
    nsmap
}

///! Produce a row representing the detected signals in the signal map
///! using `s` as the starting row inclusive and `e` as the ending row
///! exclusive.
pub fn single(smap: &SignalMap, s: usize, e: usize) -> Vec<f64> {            
    let mut results: Vec<f64> = Vec::new();

    let mut min = std::f64::MAX;
    let mut max = std::f64::MIN;

    // Filter the output to bring out potential signals and eliminate noise.
    for x in 0..smap.w {
        let mut dsum = 0f64;
        let mut last0 = 0f64;
        
        for y in s..e {
            let lpos = (y * smap.w + x) as isize - 1;
            let rpos = (y * smap.w + x) as isize + 1;
            
            let l = if lpos > 0 { smap.v[lpos as usize] } else { 0f64 };
            let r = if rpos < smap.v.len() as isize { smap.v[rpos as usize] } else { 0f64 };
               
            let c = smap.v[y * smap.w + x];
        
            dsum += (c - last0).abs() + (c - l) + (c - r);
            
            last0 = c;
        }
        
        // dsum = (dsum / smap.h as f64).log(10f64);
        dsum = dsum; // / smap.h as f64;
                
        if dsum > max {
            max = dsum;
        }
        
        if dsum < min {
            min = dsum;
        }
        
        results.push(dsum);
    }
    
    // Normalize the `dsum` to the domain of 0.0 to 1.0.
    for x in 0..results.len() {
        results[x] = 1.0 - (results[x] - min) / (max - min); 
    }    
    results
}
