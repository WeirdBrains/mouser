use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyboardProtocolState {
    pub enabled: bool,
    pub progressive_flags: u32,
}

impl KeyboardProtocolState {
    pub fn negotiation_response(&self) -> &'static str {
        // Minimal ok response: declare support for CSI u reports
        "\x1b[?u"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KittyGraphicsEvent {
    Begin { id: Option<u32>, width: Option<u32>, height: Option<u32> },
    DataChunk(Vec<u8>),
    End,
    Delete { id: Option<u32> },
}

pub fn parse_kitty_graphics_params(params: &str) -> KittyGraphicsEvent {
    // Very small parser: recognize t=d (delete) and i=<id>, s=<w>, v=<h>
    let mut id = None;
    let mut width = None;
    let mut height = None;
    let mut delete = false;
    for kv in params.split(',') {
        let mut it = kv.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        match k {
            "i" => id = v.parse::<u32>().ok(),
            "s" => width = v.parse::<u32>().ok(),
            "v" => height = v.parse::<u32>().ok(),
            "t" if v == "d" => delete = true,
            _ => {}
        }
    }
    if delete {
        KittyGraphicsEvent::Delete { id }
    } else {
        KittyGraphicsEvent::Begin { id, width, height }
    }
}

