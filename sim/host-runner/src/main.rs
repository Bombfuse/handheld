use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use handheld_cart::CartReader;
use minifb::{Key, Scale, Window, WindowOptions};
use std::sync::Mutex as StdMutex;
use std::sync::{Arc, Mutex};
use wasmtime::*;

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const FRAMEBUFFER_OFFSET: usize = 0x00300;
const FRAMEBUFFER_SIZE: usize = WIDTH * HEIGHT;
const PALETTE_OFFSET: usize = 0x00000;
const INPUT_OFFSET: usize = 0x00200;
const SAMPLE_RATE: f32 = 44100.0;

// ======================== Audio ========================

#[derive(Clone)]
struct ToneChannel {
    frequency: f32, freq_end: f32, duration_samples: u32, elapsed_samples: u32,
    volume: f32, waveform: u8, active: bool, slide: bool, phase: f32, noise_state: u32,
}

impl ToneChannel {
    fn new() -> Self {
        Self { frequency: 0.0, freq_end: 0.0, duration_samples: 0, elapsed_samples: 0,
               volume: 0.0, waveform: 0, active: false, slide: false, phase: 0.0, noise_state: 1 }
    }
    fn next_sample(&mut self) -> f32 {
        if !self.active { return 0.0; }
        if self.elapsed_samples >= self.duration_samples { self.active = false; return 0.0; }
        let freq = if self.slide && self.duration_samples > 0 {
            self.frequency + (self.freq_end - self.frequency) * (self.elapsed_samples as f32 / self.duration_samples as f32)
        } else { self.frequency };
        let period = SAMPLE_RATE / freq.max(1.0);
        let sample = match self.waveform {
            0 => if self.phase < 0.25 { 1.0 } else { -1.0 },
            1 => if self.phase < 0.5 { 1.0 } else { -1.0 },
            2 => if self.phase < 0.5 { 4.0 * self.phase - 1.0 } else { 3.0 - 4.0 * self.phase },
            3 => { let step = freq / SAMPLE_RATE;
                   if self.phase + step >= 1.0 { let bit = ((self.noise_state >> 1) ^ self.noise_state) & 1; self.noise_state = (self.noise_state >> 1) | (bit << 14); }
                   if self.noise_state & 1 == 1 { 1.0 } else { -1.0 } },
            4 => 2.0 * self.phase - 1.0,
            _ => 0.0,
        };
        self.phase += 1.0 / period;
        if self.phase >= 1.0 { self.phase -= 1.0; }
        self.elapsed_samples += 1;
        sample * self.volume
    }
}

type AudioState = Arc<Mutex<[ToneChannel; 4]>>;

fn setup_audio() -> Result<(AudioState, cpal::Stream)> {
    let channels: AudioState = Arc::new(Mutex::new([ToneChannel::new(), ToneChannel::new(), ToneChannel::new(), ToneChannel::new()]));
    let host = cpal::default_host();
    let device = host.default_output_device().context("No audio output device")?;
    let config = cpal::StreamConfig { channels: 1, sample_rate: cpal::SampleRate(SAMPLE_RATE as u32), buffer_size: cpal::BufferSize::Default };
    let ch = channels.clone();
    let stream = device.build_output_stream(&config, move |data: &mut [f32], _| {
        let mut chans = ch.lock().unwrap();
        for s in data.iter_mut() { let mut mix = 0.0f32; for c in chans.iter_mut() { mix += c.next_sample(); } *s = (mix / 4.0).clamp(-1.0, 1.0); }
    }, |e| eprintln!("Audio error: {e}"), None)?;
    stream.play()?;
    Ok((channels, stream))
}

// ======================== Runtime State ========================

struct LoadedCart {
    name: String,
    data: Vec<u8>,
}

enum RuntimeAction {
    LaunchGame(usize),
    ReturnToLauncher,
}

struct RuntimeState {
    carts: Vec<LoadedCart>,
    pending_action: Option<RuntimeAction>,
}

// ======================== Display ========================

fn rgb565_to_rgb888(rgb565: u16) -> u32 {
    let r = ((rgb565 >> 11) & 0x1F) as u32;
    let g = ((rgb565 >> 5) & 0x3F) as u32;
    let b = (rgb565 & 0x1F) as u32;
    ((r * 255 / 31) << 16) | ((g * 255 / 63) << 8) | (b * 255 / 31)
}

fn read_input(window: &Window) -> u16 {
    let mut b: u16 = 0;
    if window.is_key_down(Key::Up) { b |= 1 << 0; }
    if window.is_key_down(Key::Down) { b |= 1 << 1; }
    if window.is_key_down(Key::Left) { b |= 1 << 2; }
    if window.is_key_down(Key::Right) { b |= 1 << 3; }
    if window.is_key_down(Key::Z) || window.is_key_down(Key::Space) { b |= 1 << 4; }
    if window.is_key_down(Key::X) { b |= 1 << 5; }
    if window.is_key_down(Key::A) { b |= 1 << 6; }
    if window.is_key_down(Key::S) { b |= 1 << 7; }
    if window.is_key_down(Key::Enter) { b |= 1 << 8; }
    if window.is_key_down(Key::RightShift) || window.is_key_down(Key::Backspace) { b |= 1 << 9; }
    if window.is_key_down(Key::Q) { b |= 1 << 10; }
    if window.is_key_down(Key::W) { b |= 1 << 11; }
    b
}

fn default_palette() -> [u32; 256] {
    let mut pal = [0u32; 256];
    pal[0] = 0x000000; pal[1] = 0xFFFFFF; pal[2] = 0x00C850; pal[3] = 0x5078FF;
    pal[4] = 0xFF5050; pal[5] = 0xFFC800; pal[6] = 0xFF00FF; pal[7] = 0x00FFFF;
    pal[8] = 0x808080; pal[9] = 0x404040; pal[10] = 0x008000; pal[11] = 0x000080;
    pal[12] = 0x800000; pal[13] = 0x808000; pal[14] = 0x800080; pal[15] = 0x008080;
    for i in 16..256 { let v = ((i - 16) as u32 * 255) / 239; pal[i] = (v << 16) | (v << 8) | v; }
    pal
}

// ======================== Module Loading ========================

fn create_linker(engine: &Engine, audio_channels: &AudioState, state: &Arc<StdMutex<RuntimeState>>) -> Result<Linker<()>> {
    let mut linker = Linker::new(engine);

    linker.func_wrap("env", "host_trace", |mut caller: Caller<'_, ()>, ptr: u32, len: u32| {
        let memory = caller.get_export("memory").unwrap().into_memory().unwrap();
        let data = memory.data(&caller);
        if let Some(slice) = data.get(ptr as usize..(ptr + len) as usize) {
            if let Ok(msg) = std::str::from_utf8(slice) { println!("[trace] {msg}"); }
        }
    })?;

    linker.func_wrap("env", "host_random", |_: Caller<'_, ()>| -> u32 {
        static mut SEED: u32 = 0;
        unsafe {
            if SEED == 0 { SEED = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos(); }
            SEED ^= SEED << 13; SEED ^= SEED >> 17; SEED ^= SEED << 5; SEED
        }
    })?;

    let audio = audio_channels.clone();
    linker.func_wrap("env", "host_tone", move |_: Caller<'_, ()>, channel: u32, frequency: u32, duration: u32, volume: u32, waveform: u32| {
        let slide = channel & 0x80 != 0;
        let ch_idx = (channel & 0x7F) as usize;
        if ch_idx >= 4 { return; }
        let mut chans = audio.lock().unwrap();
        let ch = &mut chans[ch_idx];
        ch.active = true; ch.elapsed_samples = 0;
        ch.duration_samples = ((duration as f32 / 1000.0) * SAMPLE_RATE) as u32;
        ch.volume = (volume as f32 / 255.0).clamp(0.0, 1.0);
        ch.waveform = waveform as u8; ch.phase = 0.0; ch.noise_state = 1;
        if slide { ch.frequency = (frequency & 0xFFFF) as f32; ch.freq_end = (frequency >> 16) as f32; ch.slide = true; }
        else { ch.frequency = frequency as f32; ch.freq_end = frequency as f32; ch.slide = false; }
    })?;

    let st = state.clone();
    linker.func_wrap("env", "host_quit", move |_: Caller<'_, ()>| {
        st.lock().unwrap().pending_action = Some(RuntimeAction::ReturnToLauncher);
    })?;

    let st = state.clone();
    linker.func_wrap("env", "host_game_count", move |_: Caller<'_, ()>| -> u32 {
        st.lock().unwrap().carts.len() as u32
    })?;

    let st = state.clone();
    linker.func_wrap("env", "host_game_name", move |mut caller: Caller<'_, ()>, index: u32, ptr: u32, len: u32| -> u32 {
        let name = st.lock().unwrap().carts.get(index as usize).map(|c| c.name.clone()).unwrap_or_default();
        let memory = caller.get_export("memory").unwrap().into_memory().unwrap();
        let bytes = name.as_bytes();
        let write_len = bytes.len().min(len as usize);
        memory.data_mut(&mut caller)[ptr as usize..ptr as usize + write_len].copy_from_slice(&bytes[..write_len]);
        write_len as u32
    })?;

    let st = state.clone();
    linker.func_wrap("env", "host_launch_game", move |_: Caller<'_, ()>, index: u32| {
        st.lock().unwrap().pending_action = Some(RuntimeAction::LaunchGame(index as usize));
    })?;

    Ok(linker)
}

struct LoadedModule {
    store: Store<()>,
    init_fn: Option<TypedFunc<(), ()>>,
    update_fn: Option<TypedFunc<(), ()>>,
    draw_fn: Option<TypedFunc<(), ()>>,
    tick_fn: Option<TypedFunc<(), ()>>,
    memory: Memory,
}

fn load_wasm(engine: &Engine, linker: &Linker<()>, wasm: &[u8]) -> Result<LoadedModule> {
    let module = Module::new(engine, wasm).context("Failed to compile WASM")?;
    let mut store = Store::new(engine, ());
    let instance = linker.instantiate(&mut store, &module).context("Failed to instantiate")?;

    let init_fn = instance.get_typed_func::<(), ()>(&mut store, "init").ok();
    let update_fn = instance.get_typed_func::<(), ()>(&mut store, "update").ok();
    let draw_fn = instance.get_typed_func::<(), ()>(&mut store, "draw").ok();
    let tick_fn = instance.get_typed_func::<(), ()>(&mut store, "_sdk_tick").ok();
    let memory = instance.get_memory(&mut store, "memory").context("No memory export")?;

    // Init default palette
    let default_pal = default_palette();
    {
        let data = memory.data_mut(&mut store);
        for (i, &color) in default_pal.iter().enumerate() {
            let r = ((color >> 16) & 0xFF) as u8;
            let g = ((color >> 8) & 0xFF) as u8;
            let b = (color & 0xFF) as u8;
            let rgb565: u16 = ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3);
            let offset = PALETTE_OFFSET + i * 2;
            data[offset] = (rgb565 & 0xFF) as u8;
            data[offset + 1] = (rgb565 >> 8) as u8;
        }
    }

    if let Some(f) = &init_fn { f.call(&mut store, ())?; }

    Ok(LoadedModule { store, init_fn, update_fn, draw_fn, tick_fn, memory })
}

// ======================== Main ========================

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Load carts
    let carts_dir = args.get(1).map(|s| s.as_str()).unwrap_or("./carts");
    let mut carts = Vec::new();

    if let Ok(entries) = std::fs::read_dir(carts_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "cart") {
                if let Ok(data) = std::fs::read(&path) {
                    if let Ok(reader) = CartReader::new(&data) {
                        let name = reader.meta().map(|m| m.name().to_string()).unwrap_or_else(|| {
                            path.file_stem().unwrap_or_default().to_string_lossy().to_string()
                        });
                        println!("Loaded cart: {name} ({})", path.display());
                        carts.push(LoadedCart { name, data });
                    }
                }
            }
        }
    }

    // Also accept a single .wasm or .cart file as argument
    if carts.is_empty() {
        if let Some(path) = args.get(1) {
            if path.ends_with(".wasm") {
                let data = std::fs::read(path)?;
                let mut writer = handheld_cart::CartWriter::new();
                writer.set_meta("Game", "Unknown", "1.0").set_wasm(&data);
                carts.push(LoadedCart { name: "Game".into(), data: writer.build() });
            } else if path.ends_with(".cart") {
                let data = std::fs::read(path)?;
                if let Ok(reader) = CartReader::new(&data) {
                    let name = reader.meta().map(|m| m.name().to_string()).unwrap_or("Game".into());
                    carts.push(LoadedCart { name, data });
                }
            }
        }
    }

    if carts.is_empty() {
        bail!("No carts found. Usage: host-runner [carts-dir | game.wasm | game.cart]");
    }

    let (audio_channels, _audio_stream) = setup_audio()?;
    let engine = Engine::default();

    let state = Arc::new(StdMutex::new(RuntimeState {
        carts,
        pending_action: None,
    }));

    let linker = create_linker(&engine, &audio_channels, &state)?;

    // Load launcher
    static LAUNCHER_WASM: &[u8] = include_bytes!("../../../games/launcher/launcher.wasm");

    let mut window = Window::new("Handheld OS", WIDTH, HEIGHT,
        WindowOptions { scale: Scale::X2, ..WindowOptions::default() })?;
    window.set_target_fps(60);

    let mut display_buf = vec![0u32; WIDTH * HEIGHT];
    let mut prev_buttons: u16 = 0;

    // Start with launcher if multiple carts, or directly with the single game
    let single_game = state.lock().unwrap().carts.len() == 1;
    let mut current = if single_game {
        let wasm = CartReader::new(&state.lock().unwrap().carts[0].data)?.wasm().context("No WASM in cart")?.to_vec();
        load_wasm(&engine, &linker, &wasm)?
    } else {
        load_wasm(&engine, &linker, LAUNCHER_WASM)?
    };
    let mut is_launcher = !single_game;

    while window.is_open() && !window.is_key_down(Key::Escape) {
        let current_buttons = read_input(&window);
        {
            let data = current.memory.data_mut(&mut current.store);
            data[INPUT_OFFSET] = (current_buttons & 0xFF) as u8;
            data[INPUT_OFFSET + 1] = (current_buttons >> 8) as u8;
            data[INPUT_OFFSET + 2] = (prev_buttons & 0xFF) as u8;
            data[INPUT_OFFSET + 3] = (prev_buttons >> 8) as u8;
        }
        prev_buttons = current_buttons;

        if let Some(f) = &current.tick_fn { f.call(&mut current.store, ())?; }
        if let Some(f) = &current.update_fn { f.call(&mut current.store, ())?; }
        if let Some(f) = &current.draw_fn { f.call(&mut current.store, ())?; }

        // Check for pending action
        let action = state.lock().unwrap().pending_action.take();
        if let Some(action) = action {
            match action {
                RuntimeAction::LaunchGame(idx) => {
                    let cart_data = state.lock().unwrap().carts[idx].data.clone();
                    let wasm = CartReader::new(&cart_data)?.wasm().context("No WASM in cart")?.to_vec();
                    current = load_wasm(&engine, &linker, &wasm)?;
                    is_launcher = false;
                }
                RuntimeAction::ReturnToLauncher => {
                    current = load_wasm(&engine, &linker, LAUNCHER_WASM)?;
                    is_launcher = true;
                }
            }
        }

        // Read palette and blit
        let mut palette = [0u32; 256];
        {
            let data = current.memory.data(&current.store);
            for i in 0..256 {
                let off = PALETTE_OFFSET + i * 2;
                let rgb565 = (data[off] as u16) | ((data[off + 1] as u16) << 8);
                palette[i] = rgb565_to_rgb888(rgb565);
            }
        }
        {
            let data = current.memory.data(&current.store);
            for i in 0..FRAMEBUFFER_SIZE {
                display_buf[i] = palette[data[FRAMEBUFFER_OFFSET + i] as usize];
            }
        }

        window.update_with_buffer(&display_buf, WIDTH, HEIGHT)?;
    }

    Ok(())
}
