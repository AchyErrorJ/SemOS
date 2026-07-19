use crate::{Result, SheafError};
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Suid {
    pub high: u64,
    pub low: u64,
}

impl Suid {
    pub fn mint() -> Result<Self> {
        let mut b = [0u8; 16];
        match File::open("/dev/urandom") {
            Ok(mut f) => f.read_exact(&mut b)?,
            Err(_) => {
                let now = crate::unix_now();
                let pid = std::process::id() as u64;
                let addr = (&b as *const u8 as usize) as u64;
                let seed = now ^ pid.rotate_left(17) ^ addr.rotate_left(31);
                b[..8].copy_from_slice(&seed.to_le_bytes());
                let mix = crate::sha256::digest(&b);
                b[8..].copy_from_slice(&mix[..8]);
            }
        }
        Ok(Self {
            high: u64::from_be_bytes(b[..8].try_into().unwrap()),
            low: u64::from_be_bytes(b[8..].try_into().unwrap()),
        })
    }
}

impl fmt::Display for Suid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}:{:016x}", self.high, self.low)
    }
}

impl FromStr for Suid {
    type Err = SheafError;
    fn from_str(s: &str) -> Result<Self> {
        let (h, l) = s.split_once(':')
            .ok_or_else(|| SheafError::Parse(format!("bad suid {s:?}")))?;
        Ok(Self {
            high: u64::from_str_radix(h, 16)
                .map_err(|_| SheafError::Parse(format!("bad suid high {h:?}")))?,
            low: u64::from_str_radix(l, 16)
                .map_err(|_| SheafError::Parse(format!("bad suid low {l:?}")))?,
        })
    }
}
