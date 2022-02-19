use log::{info};
use std::error::Error;
use std::path::{PathBuf};
use std::cell::{RefCell};
use std::rc::{Rc};
use std::collections::{HashMap, HashSet};
use std::fs::{OpenOptions, File};
use std::cmp::{Eq};
use std::hash::{Hash};
use regex::{Regex};

use crate::update_detector::{RegionTimestamps};
use crate::update_detector::{CLoc, RLoc};
use crate::dim_renderer::{DimensionRenderer};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type ShareHashMap<K, V> = Rc<RefCell<HashMap<K, V>>>;
type ShareHashSet<T> = Rc<RefCell<HashSet<T>>>;

pub struct Dimension {
    pub dim_path: PathBuf,
    pub cache_path: PathBuf,
    pub timestamps: HashMap<RLoc, RegionTimestamps>,
    pub render_regions: HashMap<RLoc, HashSet<CLoc>>,
}

fn to_cache_name(loc: &RLoc) -> String {
    format!("r.{:0}.{:0}.cache", loc.0, loc.1)
}

fn share_borrow_mut_with<K: Eq + Hash, V, F: FnOnce() -> Rc<V>>(hash_map: &ShareHashMap<K, Rc<V>>, key: K, default: F) -> Rc<V> {
    let map = Rc::clone(hash_map);
    let mut map_m = map.borrow_mut();
    let e = map_m.entry(key).or_insert_with(default);
    Rc::clone(&e)
}

impl Dimension {
    pub fn from_dimdir(dim_path: &PathBuf, cache_path: &PathBuf) -> Result<Dimension> {
        // Read regions
        let mut region_locs: HashMap<RLoc, PathBuf> = Default::default();
        let dir = dim_path.read_dir()?;
        let region_re = Regex::new(r"r\.(-?\d+)\.(-?\d+)\.mca").unwrap();
        for entry in dir {
            let file = entry?;
            if file.path().is_dir() { continue; }

            let filestr = file.file_name().into_string().unwrap();
            let caps = region_re.captures(&filestr);
            if let None = caps { continue; }
            let caps = caps.unwrap();

            let x: i32 = caps.get(1).unwrap().as_str().parse().unwrap();
            let z: i32 = caps.get(2).unwrap().as_str().parse().unwrap();
            region_locs.insert(RLoc(x, z), file.path());
        }

        // Get chunk timestamps for regions and caches
        let mut timestamps: HashMap<RLoc, RegionTimestamps> = Default::default();
        let render_regions: ShareHashMap<RLoc, ShareHashSet<CLoc>> = Default::default();
        for (rloc, path) in region_locs {
            let mut region_file = File::open(&path).unwrap();
            let region = RegionTimestamps::from_regiondata(&mut region_file)?;

            let mut cache_path = PathBuf::from(&cache_path);
            cache_path.push(to_cache_name(&rloc));
            let cache = match File::open(&cache_path) {
                Ok(mut cache_file) => {
                    // info!("cache path {}", std::fs::canonicalize(&cache_path).unwrap().to_str().unwrap());
                    info!("cache OK {}", cache_path.to_str().unwrap());
                    Some(RegionTimestamps::from_cachedata(&mut cache_file).unwrap())
                },
                Err(_) => None,
            };

            // If cache not exists, pass None.
            let diff = region.diffs(cache.as_ref())?;

            if diff.len() == 0 {
                continue;
            }
            timestamps.insert(rloc.clone(), region);

            // Get render chunks hashset for the region.
            let render_required_chunks_r = share_borrow_mut_with(&render_regions, rloc.clone(), || Default::default());
            let mut render_required_chunks = render_required_chunks_r.borrow_mut();
            for cloc_tuple in diff {
                let cloc = CLoc::from(cloc_tuple);
                render_required_chunks.insert(cloc.clone());
                
                // Set South chunk
                if cloc.1 <= 15 {
                    // in the region
                    let south = CLoc::from(cloc).offset(0, 1);
                    render_required_chunks.insert(south);
                } else {
                    // In South region
                    // Convert chunk location for south region.
                    let south = cloc.offset(0, -16);
                    // Get render chunks hashset for south region.
                    let render_required_chunks_south_r = share_borrow_mut_with(
                        &render_regions, rloc.offset(0, 1), || Default::default());
                    let mut render_required_chunks_south = render_required_chunks_south_r.borrow_mut();
                    render_required_chunks_south.insert(south);
                }
            }
        }


        // render_regions: ShareHashMap<RLoc, ShareHashSet<CLoc>>,
        // Rc<RefCell<HashMap<RLoc, Rc<RefCell<HashSet>>>>>
        // to HashMap<RLoc, HashSet>
        let render_regions = {
            let mut regions = render_regions.borrow_mut();
            let mut new_render_regions: HashMap<RLoc, HashSet<CLoc>> = Default::default();
            let rlocs: Vec<RLoc> = regions.keys().cloned().collect();

            rlocs.iter().for_each(|rloc| {
                let region = regions.remove(&rloc).unwrap();
                let region = Rc::try_unwrap(region).ok().unwrap().into_inner();
                new_render_regions.insert(rloc.clone(), region);
            });

            new_render_regions
        };
        info!("render_regions count: {}", render_regions.keys().len());

        Ok(Dimension {
            dim_path: dim_path.to_path_buf(),
            cache_path: cache_path.to_path_buf(),
            timestamps: timestamps,
            render_regions: render_regions,
        })
    }
    pub fn save_cache(&self) -> std::io::Result<()> {
        for (rloc, timestamps) in &self.timestamps {
            info!("save {} {}", rloc.0, rloc.1);
            let filepath = self.cache_path.join(to_cache_name(&rloc));
            let mut file = OpenOptions::new()
                            .write(true)
                            .create(true)
                            .open(filepath)?;
            timestamps.save_cache(&mut file)?;
        }
        Ok(())
    }
}

pub fn _sample1() {
    let dim_path = std::path::PathBuf::from(r"/Users/sohei/Library/Application Support/minecraft/saves/with chizuru/region/");
    let cache_path = std::path::PathBuf::from(r"./cache/");
    let image_path = std::path::PathBuf::from(r"./images/");
    let dim = Dimension::from_dimdir(&dim_path, &cache_path).unwrap();

    use std::sync::{Arc};
    let palette = Arc::new(crate::renderer::get_palette(Some(r"palette.tar.gz")).unwrap());
    let dim_renderer = DimensionRenderer::new(dim, &image_path);

    use indicatif::{ProgressBar, MultiProgress, ProgressStyle};
    use std::sync::mpsc::{sync_channel};

    let (progress_sender, progress_receiver) = sync_channel(10);

    let render_handle = std::thread::spawn(move || {
        dim_renderer.render_all(palette, progress_sender);
        dim_renderer.get_dimension().save_cache().unwrap();
    });

    let multi_bar = Arc::new(MultiProgress::new());
    let mut bars: Vec<ProgressBar> = Default::default();
    let bar_master = multi_bar.add(ProgressBar::new(0));
    let sty_master = ProgressStyle::default_bar()
        .template("[{elapsed_precise}] {bar:40.cyan/cyan} {pos:>7}/{len:7} {msg}");
        //.progress_chars("##-");
    bar_master.set_style(sty_master.clone());
    bar_master.set_message("Total");
    let sty = ProgressStyle::default_bar()
        .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}")
        .progress_chars("##-");
    for _ in 0..4 {
        let bar = multi_bar.add(ProgressBar::new(0));
        bar.set_style(sty.clone());
        bar.inc(1);
        bars.push(bar);
    }

    let progress_handle = std::thread::spawn(move || {
        let mut bar_map: HashMap<RLoc, usize> = Default::default();
        let mut uses: Vec<bool> = vec![false; 4];
        for progress in progress_receiver {
            use crate::dim_renderer::RegionProgress::*;
            match progress {
                Begin(rloc, max) => {
                    info!("Begin {},{} {}", rloc.0, rloc.1, max);

                    let idx = uses.iter().enumerate().find_map(|(idx, flag)| {
                        if !flag {
                            Some(idx)
                        } else { None }
                    }).unwrap();
                    info!("index: {:?}", idx);
                    uses[idx] = true;
                    bar_map.insert(rloc.clone(), idx);
                    bars[idx].set_length(max as u64);
                    bars[idx].set_position(0);
                    bars[idx].reset_elapsed();
                    bars[idx].set_message(format!("({:3},{:3})", rloc.0, rloc.1))
                },
                Step(rloc) => {
                    let idx = bar_map.get(&rloc).unwrap();
                    bars[*idx].inc(1);
                    bar_master.inc(1);
                },
                End(rloc) => {
                    info!("  End {},{}", rloc.0, rloc.1);
                    let idx = bar_map.get(&rloc).unwrap();
                    bars[*idx].finish_with_message(format!("({:3},{:3}) OK", rloc.0, rloc.1));
                    uses[*idx] = false;
                    bar_map.remove(&rloc);
                },
                BeginAll(max) => {
                    bar_master.set_length(max as u64);
                },
                EndAll => {
                    bar_master.finish_with_message("Total OK");
                }
            };
        }
    });
    info!("multi bar join");

    std::thread::sleep(std::time::Duration::from_millis(100));
    multi_bar.join().unwrap();
    info!("process handle join");
    progress_handle.join().unwrap();
    render_handle.join().unwrap();
}
