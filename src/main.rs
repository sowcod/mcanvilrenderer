mod renderer;
mod update_detector;
mod dimension;
mod dim_renderer;

use log::info;
use std::collections::HashMap;
use std::path::PathBuf;
use std::error::Error;
use regex::Regex;
use lazy_static::lazy_static;

use update_detector::{RLoc, RegionBounds};
use dim_renderer::DimensionRenderer;
use dim_renderer::RegionProgress::*;
use dimension::Dimension;
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;
use clap::{Parser, ArgEnum};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// World path
    #[clap(short, long, value_name="DIR", parse(from_os_str))]
    dimension_path: PathBuf,

    /// Cache path
    #[clap(short, long, value_name="DIR", parse(from_os_str))]
    cache_path: PathBuf,

    /// Image path
    #[clap(short, long, value_name="DIR", parse(from_os_str))]
    image_path: PathBuf,

    /// Palette path
    #[clap(short, long, value_name="DIR", parse(from_os_str))]
    palette_path: PathBuf,

    // Render location range.(Set one or two locations. example: "L-1,10" or "L-10,10" "L10,20")
    #[clap(short='R', long, parse(try_from_str = parse_location_val), multiple_occurrences(true), max_occurrences(2))]
    range: Option<Vec<(i32, i32)>>,

    // Log mode
    #[clap(short, long)]
    bgmode: bool,

    // cache mode
    #[clap(long, arg_enum, default_value_t = CacheMode::Default)]
    cache_mode: CacheMode,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ArgEnum)]
enum CacheMode {
    Default, // cache SAVE and LOAD
    Refresh, // cache SAVE only
    ReadOnly, // cache LOAD only
    NoCache, // ignore cache
}

/// Parse location value
fn parse_location_val(s: &str) -> Result<(i32, i32), Box<dyn Error + Send + Sync + 'static>>
{
    lazy_static! {
        static ref RE: Regex = Regex::new(r"(-?\d+),(-?\d+)").unwrap();
    }
    if let Some(cap) = RE.captures(s) {
       let x: i32 = cap.get(1).unwrap().as_str().parse().unwrap();
       let z: i32 = cap.get(2).unwrap().as_str().parse().unwrap();
       return Ok((x, z));
    } else {
        return Err(format!("invalid xloc,zloc").into());
    }
}

/*
> RUST_LOG=info cargo run
*/
fn main() {
    env_logger::init();

    let args = Cli::parse();

    let bounds: Option<RegionBounds>;
    if let Some(range) = args.range {
        match range.len() {
            1 => {
                let range = range[0];
                bounds = Some((RLoc(range.0, range.1), RLoc(range.0, range.1)));
            },
            2 => {
                let range1 = range[0];
                let range2 = range[1];
                bounds = Some((
                    RLoc(range1.0.min(range2.0), range1.1.min(range2.1)),
                    RLoc(range1.0.max(range2.0), range1.1.max(range2.1)),
                ));
            },
            _ => {
                bounds = None;
            }
        } 
    } else {
        bounds = None;
    }

    let nocache = args.cache_mode == CacheMode::NoCache || args.cache_mode == CacheMode::Refresh;
    let cache_ro = args.cache_mode == CacheMode::ReadOnly;
    let dim = Dimension::from_dimdir(&args.dimension_path, &args.cache_path, bounds.as_ref(), nocache, cache_ro).unwrap();

    let palette = Arc::new(crate::renderer::get_palette(&args.palette_path).unwrap());
    let dim_renderer = DimensionRenderer::new(dim, &args.image_path);

    let (progress_sender, progress_receiver) = sync_channel(10);

    let render_handle = std::thread::spawn(move || {
        dim_renderer.render_all(palette, progress_sender, nocache);
    });

    if args.bgmode {
        bg_mode(progress_receiver);
    } else {
        normal_mode(progress_receiver);
    }
    
    render_handle.join().unwrap();
}

fn normal_mode(receiver: Receiver<dim_renderer::RegionProgress>) {
    use indicatif::{ProgressBar, MultiProgress, ProgressStyle};

    let multi_bar = Arc::new(MultiProgress::new());
    let mut bars: Vec<ProgressBar> = Default::default();
    let bar_master = multi_bar.add(ProgressBar::new(0));
    let sty_master = ProgressStyle::default_bar()
        .template("[{elapsed_precise}] {bar:40.cyan/cyan} {pos:>7}/{len:7} {msg} ETA: [{eta_precise}]");
        //.progress_chars("##-");
    bar_master.set_style(sty_master.unwrap());
    bar_master.set_message("Total");
    let sty = ProgressStyle::default_bar()
        .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} Region: {msg}")
        .unwrap()
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
        for progress in receiver {
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

    progress_handle.join().unwrap();
}

fn bg_mode(receiver: Receiver<dim_renderer::RegionProgress>) {
    for progress in receiver {
        match progress {
            Begin(rloc, max) => {
                println!("Begin region:({}, {}) / chunks: {}", rloc.0, rloc.1, max);
            },
            Step(_) => (),
            End(rloc) => {
                println!("  End region:({}, {})", rloc.0, rloc.1);
            },
            BeginAll(max) => {
                println!("Begin total chunks: {}", max);
            },
            EndAll => {
                println!("  End all.");
            }
        }
    }
}