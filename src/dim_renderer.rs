use fastanvil::{Region, RegionLoader, RegionFileLoader, JavaChunk, TopShadeRenderer};
use std::collections::{HashMap, HashSet};
use std::mem::drop;
use std::sync::{Arc, Mutex, RwLock, mpsc::SyncSender};
use log::{info, debug};
use std::path::{Path, PathBuf};
use threadpool::{ThreadPool};
use image::{ImageBuffer, Rgba};
use slice_of_array::prelude::*;
use crate::dimension::Dimension;
use crate::update_detector::{RLoc, CLoc, r2r, c2c};

type ShareRegion = Arc<Box<dyn Region<JavaChunk>>>;
type ChunkImageBuffer = [fastanvil::Rgba; 16*16];

pub fn to_image_name(rloc: &RLoc) -> String {
    format!("r.{:0}.{:0}.png", rloc.0, rloc.1)
}

pub enum RegionProgress {
    BeginAll(usize),
    EndAll,
    Begin(RLoc, usize),
    Step(RLoc),
    // Error(RLoc),
    End(RLoc),
}

struct DimensionRendererInner {
    image_path: PathBuf,
    loader: RegionFileLoader<JavaChunk>,
    dimension: Box<Dimension>,
    regions: Arc<Mutex<HashMap<RLoc, ShareRegion>>>,
    chunks: Arc<RwLock<HashMap<(RLoc, CLoc), Arc<JavaChunk>>>>,
}

pub struct DimensionRenderer {
    inner: Arc<DimensionRendererInner>,
}

impl DimensionRenderer {
    fn get_region(inner: &DimensionRendererInner, rloc: &RLoc) -> Option<ShareRegion> {
        let mut regions_l = inner.regions.lock().unwrap();

        regions_l.get(&rloc).map(|r| Arc::clone(&r)).or_else(|| {
            let region = Arc::new(inner.loader.region(r2r(rloc.0), r2r(rloc.1))?);
            regions_l.insert(rloc.clone(), Arc::clone(&region));
            Some(region)
        })
    }

    fn get_chunk(inner: &DimensionRendererInner, rloc: &RLoc, cloc: &CLoc) -> Option<Arc<JavaChunk>> {
        let key = (rloc.clone(), cloc.clone());
        let chunks_r = Arc::clone(&inner.chunks);
        let chunks_rl = chunks_r.read().unwrap();
        let chunk = chunks_rl.get(&key);
        if let None = chunk {
            drop(chunks_rl);
            let mut chunks_wl = chunks_r.write().unwrap();
            if let Some(chunk) = chunks_wl.get(&key) {
                return Some(Arc::clone(&chunk));
            }
            let region = Self::get_region(inner, rloc);
            if let None = region { return None; }

            let new_chunk = region.unwrap().chunk(c2c(cloc.0), c2c(cloc.1));
            if let None = new_chunk { return None; }
            let new_insert_chunk = Arc::new(new_chunk.unwrap());
            chunks_wl.insert(key, Arc::clone(&new_insert_chunk));

            return Some(new_insert_chunk);
        }
        chunk.map(|c| Arc::clone(&c))
    }

    pub fn new(dimension: Dimension, image_path: &Path) -> Self {
        DimensionRenderer {
            inner: Arc::new(DimensionRendererInner {
                image_path: PathBuf::from(image_path),
                loader: RegionFileLoader::<JavaChunk>::new(dimension.dim_path.clone()),
                dimension: Box::new(dimension),
                regions: Default::default(),
                chunks: Default::default(),
            }),
        }
    }

    fn render_region(inner: &DimensionRendererInner, rloc: &RLoc, buf: Vec<fastanvil::Rgba>, palette: Arc<fastanvil::RenderedPalette>, sender: SyncSender<RegionProgress>) -> Vec<fastanvil::Rgba> {
        let clocs = if let Some(clocs) = inner.dimension.render_regions.get(rloc) {
            clocs
        } else {
            return buf;
        };
        sender.send(RegionProgress::Begin(rloc.clone(), clocs.len())).unwrap();
        
        info!("render_region clocs:{:?}", clocs.len());
        let mut buf = buf;
        let buf_l = buf.as_mut_slice();
        for cloc in clocs {
            // if cloc.0 != 15 || cloc.1 != 16 { continue; }
            let renderer = TopShadeRenderer::new(&*palette, fastanvil::HeightMode::Trust);
            if let Some(chunk_buf) = Self::render_chunk(&inner, &renderer, &rloc, &cloc) {
                for y in 0..16 {
                    let px = (cloc.0 * 16) as usize;
                    let py = (cloc.1 * 16 + y) as usize;

                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            &chunk_buf[(y * 16) as usize],
                            &mut buf_l[py * 512 + px],
                            16);
                    }
                }
            }
            sender.send(RegionProgress::Step(rloc.clone())).unwrap();
        }
        return buf;
    }

    fn render_chunk<'b>(inner: &DimensionRendererInner, renderer: &TopShadeRenderer<'b, fastanvil::RenderedPalette>, rloc: &RLoc, cloc: &CLoc) -> Option<ChunkImageBuffer> {
        let chunk = Self::get_chunk(inner, rloc, &cloc);
        if let None = chunk { return None; }

        // get north chunk
        let chunk_north = if cloc.1 == 0 {
            Self::get_chunk(inner, &rloc.offset(0, -1), &cloc.offset(0, 31))
        } else {
            Self::get_chunk(inner, rloc, &cloc.offset(0, -1))
        };

        if let Some(chunk_north) = chunk_north {
            return Some(renderer.render(&*chunk.unwrap(), Some(&*chunk_north)));
        } else {
            return Some(renderer.render(&*chunk.unwrap(), None));
        };
    }

    fn load_cached_image(inner: &DimensionRendererInner, rloc: &RLoc) -> Vec<fastanvil::Rgba> {
        let image = if let Ok(image) = image::open(inner.image_path.join(to_image_name(rloc))) {
            image
        } else {
            return vec![[0u8;4]; 512*512];
        };

        use slice_of_array::prelude::*;
        match image {
            image::DynamicImage::ImageRgba8(image) => {
                return Vec::from(image.into_vec().as_slice().nest::<[_; 4]>());
            },
            _ => {
                return vec![[0u8;4]; 512*512];
            }
        }
    }

    pub fn render_all(&self, palette: Arc<fastanvil::RenderedPalette>, sender: SyncSender<RegionProgress>, nocache: bool) {
        use std::iter::FromIterator;
        sender.send(RegionProgress::BeginAll(self.inner.dimension.render_regions.iter().fold(0, |c, (_, v)| c + v.len()))).unwrap();
        let regions = self.inner.dimension.render_regions.keys();
        let regions_remind = HashSet::<RLoc>::from_iter(regions.clone().map(Clone::clone).collect::<Vec<_>>());
        let regions_remind = Arc::new(Mutex::new(regions_remind));
        let pool = ThreadPool::new(4);
        for rloc in regions {
            let inner = Arc::clone(&self.inner);
            let rloc = rloc.clone();
            let regions_remind = Arc::clone(&regions_remind);
            let palette = Arc::clone(&palette);
            let sender = sender.clone();
            pool.execute(move || {
                // Load cached image.
                let cached_image = if nocache { vec![[0u8;4]; 512*512] }
                    else { Self::load_cached_image(&inner, &rloc) };
                // Render the region
                let new_image = Self::render_region(&inner, &rloc, cached_image, palette, sender.clone());

                // Unload chunks.
                let north_region = rloc.offset(0, -1);
                let south_region = rloc.offset(0, 1);
                let exist_north: bool;
                let exist_south: bool;
                {
                    let mut regions_remind_l = regions_remind.lock().unwrap();
                    regions_remind_l.remove(&rloc);
                    exist_north = regions_remind_l.contains(&north_region);
                    exist_south = regions_remind_l.contains(&south_region);
                }

                {
                    let mut chunks_l = inner.chunks.write().unwrap();
                    chunks_l.retain(|(c_rloc, c_cloc), _| {
                            !(
                                (c_rloc == &rloc && c_cloc.1 < 15) || // current region chunk exept bottom
                                (c_rloc == &rloc && c_cloc.1 == 15 && !exist_south) || // bottom
                                (c_rloc == &north_region && !exist_north)
                            )
                        });
                }
                // save region image
                let flat_buf: &[u8] = new_image.as_slice().flat();
                let bufvec: Vec<u8> = Vec::from(flat_buf);
                let write_path = inner.image_path.join(to_image_name(&rloc));
                let imgbuf: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(512, 512, bufvec).unwrap();

                info!("{:?}", write_path.to_str());
                imgbuf.save(write_path).unwrap();
                
                // save cache
                inner.dimension.save_cache(&rloc).unwrap();

                sender.send(RegionProgress::End(rloc.clone())).unwrap();
            });
        }
        pool.join();
        sender.send(RegionProgress::EndAll).unwrap();
    }
}
