use std::io::{Read, Write, Seek, SeekFrom, Cursor};
use std::cmp::{PartialEq};
use std::clone::{Clone};

pub type RCoord = i32;
pub type CCoord = i32;
pub fn r2r(r: RCoord) -> fastanvil::RCoord { fastanvil::RCoord(r as isize) }
pub fn c2c(c: CCoord) -> fastanvil::CCoord { fastanvil::CCoord(c as isize) }

#[derive(Hash, Eq, PartialEq, Clone)]
pub struct CLoc(pub CCoord, pub CCoord);

#[derive(Hash, Eq, PartialEq, Clone)]
pub struct RLoc(pub RCoord, pub RCoord);

impl From<(CCoord, CCoord)> for CLoc {
    fn from(tuple: (CCoord, CCoord)) -> Self {
        Self(tuple.0, tuple.1)
    }
}
impl From<(RCoord, RCoord)> for RLoc {
    fn from(tuple: (RCoord, RCoord)) -> Self {
        Self(tuple.0, tuple.1)
    }
}

impl CLoc {
    pub fn offset(&self, x: CCoord, z: CCoord) -> CLoc {
        CLoc(self.0 + x, self.1 + z)
    }
}

impl RLoc {
    pub fn offset(&self, x: RCoord, z: RCoord) -> Self {
        Self(self.0 + x, self.1 + z)
    }
}

pub struct RegionTimestamps {
    pub rawdata: [u8; 4096],
}
#[derive(Debug)]
pub struct ChunkTimestamp {
    pub x: CCoord,
    pub z: CCoord,
    pub timestamp: u32,
}
impl std::fmt::Display for ChunkTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let time = chrono::NaiveDateTime::from_timestamp(self.timestamp.into(), 0);
        write!(f, "x:{:0}, z:{:0}, timestamp:{}", self.x as i32, self.z, time.format("%Y-%m-%d %H:%M:%S"))
    }
}

impl RegionTimestamps {
    pub fn from_regiondata<T: Read + Seek>(region_data: &mut T) -> std::io::Result<Self> {
        region_data.seek(SeekFrom::Start(4096))?;
        Self::new(region_data)
    }
    pub fn from_cachedata<T: Read>(cache_data: &mut T) -> std::io::Result<Self> {
        Self::new(cache_data)
    }
    pub fn new<T: Read>(region_data: &mut T) -> std::io::Result<Self> {
        let mut rawdata: [u8; 4096] = [0; 4096];
        region_data.read_exact(&mut rawdata)?;
        Ok(RegionTimestamps {
            rawdata: rawdata
        })
    }
    pub fn save_cache<T: Write>(&self, writable: &mut T) -> std::io::Result<()> {
        writable.write_all(&self.rawdata)
    }
    #[allow(dead_code)]
    pub fn list_timestamps(&self) -> std::io::Result<Box<Vec<ChunkTimestamp>>> {
        let tsarray = self.to_tsarray()?;
        let mut timestamps: Vec<ChunkTimestamp> = Vec::new();
        for (index, timestamp) in tsarray.iter().enumerate() {
            if *timestamp > 0 {
                timestamps.push(ChunkTimestamp{
                    x: (index as i32 % 32).into(),
                    z: (index as i32 / 32).into(),
                    timestamp: *timestamp,
                });
            }

        }
        Ok(Box::new(timestamps))
    }
    pub fn to_tsarray(&self) -> std::io::Result<[u32; 1024]> {
        let mut cursor = Cursor::new(&self.rawdata);
        let mut ar: [u32; 1024] = [0; 1024];
        for index in 0..1024 {
            let mut time_buf: [u8; 4] = [0; 4];
            cursor.read_exact(&mut time_buf)?;
            let timestamp = u32::from_be_bytes(time_buf);
            ar[index] = timestamp;
        }
        Ok(ar)
    }
    pub fn diffs(&self, other: Option<&Self>) -> std::io::Result<Vec<(CCoord, CCoord)>> {
        let me_ar = self.to_tsarray()?;
        let other_ar = match other {
            Some(other) => other.to_tsarray()?,
            None => [0; 1024],
        };
        let mut diffs = Vec::new();
        for index in 0..1024 {
            if me_ar[index] > 0 && (me_ar[index] != other_ar[index]) {
                diffs.push((
                    (index as i32 % 32).into(), // x
                    (index as i32 / 32).into()  // z
                ))
            }
        }
        Ok(diffs)
    }
}

impl PartialEq for RegionTimestamps {
    fn eq(&self, other: &Self) -> bool {
        self.rawdata == other.rawdata
    }
}