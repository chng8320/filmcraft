//! Which processor runs a speech model: `auto` (a GPU when this build supports one and the
//! machine has it, else the CPU), `cpu`, `gpu` (any GPU; an error without one, never a silent
//! CPU run), `cuda[:N]` (NVIDIA, feature `cuda`) or `metal[:N]` (Apple, feature `metal`).
//! A server sets its default with `FILMCRAFT_DEVICE`; a command's `device` parameter wins.

use std::fmt;
use std::str::FromStr;

/// Where speech inference runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceChoice {
    #[default]
    Auto,
    Cpu,
    Gpu,
    Cuda(usize),
    Metal(usize),
}

impl DeviceChoice {
    /// `FILMCRAFT_DEVICE` when it is set and valid, else `Auto`.
    pub fn from_env() -> Self {
        std::env::var("FILMCRAFT_DEVICE").ok().and_then(|v| v.parse().ok()).unwrap_or_default()
    }
}

impl FromStr for DeviceChoice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim().to_ascii_lowercase();
        let (kind, n) = match s.split_once(':') {
            Some((k, n)) => (k.to_string(), Some(n.parse::<usize>().map_err(|_| format!("device {s:?}: {n:?} is not a GPU number"))?)),
            None => (s.clone(), None),
        };
        match (kind.as_str(), n) {
            ("auto", None) => Ok(Self::Auto),
            ("cpu", None) => Ok(Self::Cpu),
            ("gpu", None) => Ok(Self::Gpu),
            ("cuda" | "nvidia", n) => Ok(Self::Cuda(n.unwrap_or(0))),
            ("metal" | "apple", n) => Ok(Self::Metal(n.unwrap_or(0))),
            _ => Err(format!("unknown device {s:?}: use auto, cpu, gpu, cuda, cuda:N, metal or metal:N")),
        }
    }
}

impl fmt::Display for DeviceChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Cpu => f.write_str("cpu"),
            Self::Gpu => f.write_str("gpu"),
            Self::Cuda(n) => write!(f, "cuda:{n}"),
            Self::Metal(n) => write!(f, "metal:{n}"),
        }
    }
}

/// The back ends this build was compiled with (the CPU is always there).
pub fn compiled_backends() -> Vec<&'static str> {
    let mut v = vec!["cpu"];
    if cfg!(feature = "cuda") {
        v.push("cuda");
    }
    if cfg!(feature = "metal") {
        v.push("metal");
    }
    v
}

/// Open the processor `choice` names, with a label saying what was opened.
#[cfg(feature = "whisper")]
pub(crate) fn open(choice: DeviceChoice) -> Result<(candle_core::Device, String), crate::SpeechError> {
    use crate::SpeechError;
    use candle_core::Device;
    fn cuda(n: usize) -> Result<(Device, String), SpeechError> {
        if !cfg!(feature = "cuda") {
            return Err(SpeechError::Unavailable("no NVIDIA GPU support: built without the `cuda` feature".into()));
        }
        Device::new_cuda(n).map(|d| (d, format!("NVIDIA GPU {n} (CUDA)"))).map_err(|e| SpeechError::Model(format!("cannot open NVIDIA GPU {n}: {e}")))
    }
    fn metal(n: usize) -> Result<(Device, String), SpeechError> {
        if !cfg!(feature = "metal") {
            return Err(SpeechError::Unavailable("no Apple GPU support: built without the `metal` feature".into()));
        }
        Device::new_metal(n).map(|d| (d, format!("Apple GPU {n} (Metal)"))).map_err(|e| SpeechError::Model(format!("cannot open Apple GPU {n}: {e}")))
    }
    fn any() -> Result<(Device, String), SpeechError> {
        if !cfg!(feature = "cuda") && !cfg!(feature = "metal") {
            return Err(SpeechError::Unavailable("no GPU support in this build".into()));
        }
        let mut why = Vec::new();
        let backends: [fn(usize) -> Result<(Device, String), SpeechError>; 2] = [cuda, metal];
        for open in backends {
            match open(0) {
                Ok(d) => return Ok(d),
                Err(e) => why.push(e.to_string()),
            }
        }
        Err(SpeechError::Model(format!("no usable GPU: {}", why.join("; "))))
    }
    match choice {
        DeviceChoice::Cpu => Ok((Device::Cpu, "CPU".into())),
        DeviceChoice::Cuda(n) => cuda(n),
        DeviceChoice::Metal(n) => metal(n),
        DeviceChoice::Gpu => any(),
        DeviceChoice::Auto => Ok(any().unwrap_or_else(|e| (Device::Cpu, format!("CPU ({e})")))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_round_trip_and_bad_ones_are_refused() {
        for s in ["auto", "cpu", "gpu", "cuda:1", "metal:0"] {
            assert_eq!(s.parse::<DeviceChoice>().map(|d| d.to_string()).as_deref(), Ok(s));
        }
        assert!("cuda:x".parse::<DeviceChoice>().is_err());
        assert!("cpu:1".parse::<DeviceChoice>().is_err());
    }
}
