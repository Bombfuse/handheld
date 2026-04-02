#![cfg_attr(not(feature = "std"), no_std)]

pub const MAGIC: [u8; 4] = *b"CART";
pub const VERSION: u8 = 1;

pub const SECTION_META: u8 = 0x01;
pub const SECTION_WASM: u8 = 0x02;
pub const SECTION_SPRITESHEET: u8 = 0x03;

#[derive(Debug)]
pub enum CartError {
    BadMagic,
    BadVersion,
    TooShort,
    BadSection,
}

impl core::fmt::Display for CartError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CartError::BadMagic => write!(f, "bad magic"),
            CartError::BadVersion => write!(f, "bad version"),
            CartError::TooShort => write!(f, "too short"),
            CartError::BadSection => write!(f, "bad section"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CartError {}

struct SectionEntry {
    section_type: u8,
    offset: u32,
    length: u32,
}

pub struct CartReader<'a> {
    data: &'a [u8],
    num_sections: u8,
}

impl<'a> CartReader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, CartError> {
        if data.len() < 8 {
            return Err(CartError::TooShort);
        }
        if data[0..4] != MAGIC {
            return Err(CartError::BadMagic);
        }
        if data[4] != VERSION {
            return Err(CartError::BadVersion);
        }
        let num_sections = data[5];
        let table_end = 8 + num_sections as usize * 12;
        if data.len() < table_end {
            return Err(CartError::TooShort);
        }
        Ok(Self { data, num_sections })
    }

    fn find_section(&self, section_type: u8) -> Option<&'a [u8]> {
        for i in 0..self.num_sections as usize {
            let base = 8 + i * 12;
            let st = self.data[base];
            let offset = u32::from_le_bytes([
                self.data[base + 4], self.data[base + 5],
                self.data[base + 6], self.data[base + 7],
            ]) as usize;
            let length = u32::from_le_bytes([
                self.data[base + 8], self.data[base + 9],
                self.data[base + 10], self.data[base + 11],
            ]) as usize;
            if st == section_type {
                if offset + length <= self.data.len() {
                    return Some(&self.data[offset..offset + length]);
                }
            }
        }
        None
    }

    pub fn meta(&self) -> Option<CartMeta<'a>> {
        self.find_section(SECTION_META).map(|raw| CartMeta { raw })
    }

    pub fn wasm(&self) -> Option<&'a [u8]> {
        self.find_section(SECTION_WASM)
    }

    pub fn spritesheet(&self) -> Option<&'a [u8]> {
        self.find_section(SECTION_SPRITESHEET)
    }
}

pub struct CartMeta<'a> {
    raw: &'a [u8],
}

impl<'a> CartMeta<'a> {
    fn get_field(&self, key: &str) -> &'a str {
        let text = core::str::from_utf8(self.raw).unwrap_or("");
        for line in text.split('\n') {
            if let Some(val) = line.strip_prefix(key).and_then(|s| s.strip_prefix('=')) {
                return val;
            }
        }
        ""
    }

    pub fn name(&self) -> &'a str {
        self.get_field("name")
    }

    pub fn author(&self) -> &'a str {
        self.get_field("author")
    }

    pub fn version_str(&self) -> &'a str {
        self.get_field("version")
    }
}

// Writer (std only)
#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "std")]
pub struct CartWriter {
    meta: std::string::String,
    wasm: std::vec::Vec<u8>,
    spritesheet: Option<std::vec::Vec<u8>>,
}

#[cfg(feature = "std")]
impl CartWriter {
    pub fn new() -> Self {
        Self {
            meta: std::string::String::new(),
            wasm: std::vec::Vec::new(),
            spritesheet: None,
        }
    }

    pub fn set_meta(&mut self, name: &str, author: &str, version: &str) -> &mut Self {
        self.meta = std::format!("name={name}\nauthor={author}\nversion={version}\n");
        self
    }

    pub fn set_wasm(&mut self, data: &[u8]) -> &mut Self {
        self.wasm = data.to_vec();
        self
    }

    pub fn set_spritesheet(&mut self, data: &[u8]) -> &mut Self {
        self.spritesheet = Some(data.to_vec());
        self
    }

    pub fn build(&self) -> std::vec::Vec<u8> {
        let num_sections = if self.spritesheet.is_some() { 3u8 } else { 2u8 };
        let table_size = num_sections as usize * 12;
        let header_size = 8 + table_size;

        let meta_bytes = self.meta.as_bytes();
        let meta_offset = header_size;
        let wasm_offset = meta_offset + meta_bytes.len();
        let sprite_offset = wasm_offset + self.wasm.len();

        let total = sprite_offset + self.spritesheet.as_ref().map_or(0, |s| s.len());
        let mut out = std::vec![0u8; total];

        // Header
        out[0..4].copy_from_slice(&MAGIC);
        out[4] = VERSION;
        out[5] = num_sections;

        // Section table
        let mut idx = 8;
        // Meta
        out[idx] = SECTION_META;
        out[idx + 4..idx + 8].copy_from_slice(&(meta_offset as u32).to_le_bytes());
        out[idx + 8..idx + 12].copy_from_slice(&(meta_bytes.len() as u32).to_le_bytes());
        idx += 12;
        // Wasm
        out[idx] = SECTION_WASM;
        out[idx + 4..idx + 8].copy_from_slice(&(wasm_offset as u32).to_le_bytes());
        out[idx + 8..idx + 12].copy_from_slice(&(self.wasm.len() as u32).to_le_bytes());
        idx += 12;
        // Spritesheet
        if let Some(ref sprites) = self.spritesheet {
            out[idx] = SECTION_SPRITESHEET;
            out[idx + 4..idx + 8].copy_from_slice(&(sprite_offset as u32).to_le_bytes());
            out[idx + 8..idx + 12].copy_from_slice(&(sprites.len() as u32).to_le_bytes());
        }

        // Payloads
        out[meta_offset..meta_offset + meta_bytes.len()].copy_from_slice(meta_bytes);
        out[wasm_offset..wasm_offset + self.wasm.len()].copy_from_slice(&self.wasm);
        if let Some(ref sprites) = self.spritesheet {
            out[sprite_offset..sprite_offset + sprites.len()].copy_from_slice(sprites);
        }

        out
    }
}
