#![no_std]

use handheld_sdk::*;

// Launcher-specific host functions (not in SDK — only the launcher uses these)
unsafe extern "C" {
    fn host_game_count() -> u32;
    fn host_game_name(index: u32, buf_ptr: *mut u8, buf_len: u32) -> u32;
    fn host_launch_game(index: u32);
}

const MAX_GAMES: usize = 16;
const MAX_NAME: usize = 32;

static mut GAME_COUNT: u32 = 0;
static mut SELECTED: u32 = 0;
static mut NAMES: [[u8; MAX_NAME]; MAX_GAMES] = [[0u8; MAX_NAME]; MAX_GAMES];
static mut NAME_LENS: [u32; MAX_GAMES] = [0; MAX_GAMES];

const COL_BG: u8 = 0;
const COL_TITLE: u8 = 5;
const COL_TEXT: u8 = 1;
const COL_SELECTED_BG: u8 = 3;
const COL_SELECTED_TEXT: u8 = 1;
const COL_BORDER: u8 = 9;

#[unsafe(no_mangle)]
pub extern "C" fn init() {
    set_palette(0, 16, 16, 32);      // Dark blue-black
    set_palette(1, 220, 220, 220);    // White
    set_palette(2, 0, 200, 80);       // Green
    set_palette(3, 60, 80, 180);      // Selection blue
    set_palette(5, 255, 200, 0);      // Gold title
    set_palette(9, 40, 40, 60);       // Border

    unsafe {
        GAME_COUNT = host_game_count();
        if GAME_COUNT > 0 {
            // Try host_game_name first (works on host-runner)
            let test_len = host_game_name(0, NAMES[0].as_mut_ptr(), MAX_NAME as u32);
            if test_len > 0 {
                NAME_LENS[0] = test_len;
                for i in 1..GAME_COUNT.min(MAX_GAMES as u32) {
                    NAME_LENS[i as usize] = host_game_name(
                        i, NAMES[i as usize].as_mut_ptr(), MAX_NAME as u32,
                    );
                }
            } else {
                // Fallback: read names from WASM memory (written by k23 runtime)
                // Format at INPUT_BASE+16: [count:u32] [len0:u8 name0:31bytes] ...
                let name_base: usize = 0x200 + 16;
                let count = mem_read_u16(name_base) as u32;
                if count > 0 { GAME_COUNT = count; }
                for i in 0..GAME_COUNT.min(MAX_GAMES as u32) {
                    let offset = name_base + 4 + (i as usize) * 32;
                    let len = mem_read(offset) as u32;
                    let len = len.min(MAX_NAME as u32 - 1);
                    for j in 0..len {
                        NAMES[i as usize][j as usize] = mem_read(offset + 1 + j as usize);
                    }
                    NAME_LENS[i as usize] = len;
                }
            }
        }
        SELECTED = 0;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn update() {
    unsafe {
        let count = GAME_COUNT;
        if count == 0 { return; }

        if button_pressed(Button::Down) {
            SELECTED = (SELECTED + 1) % count;
        }
        if button_pressed(Button::Up) {
            SELECTED = if SELECTED == 0 { count - 1 } else { SELECTED - 1 };
        }
        if button_pressed(Button::A) || button_pressed(Button::Start) {
            host_launch_game(SELECTED);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn draw() {
    clear(COL_BG);

    // Title
    text("HANDHELD OS", 104, 20, COL_TITLE);
    text("Select a Game", 98, 36, COL_TEXT);

    // Border
    rect(20, 55, 280, 170, COL_BORDER);

    unsafe {
        let count = GAME_COUNT.min(MAX_GAMES as u32);
        for i in 0..count {
            let y = 65 + (i as i32) * 20;
            let is_sel = i == SELECTED;

            if is_sel {
                rect_fill(22, y - 2, 276, 18, COL_SELECTED_BG);
                text(">", 26, y, COL_SELECTED_TEXT);
            }

            let name_len = NAME_LENS[i as usize].min(MAX_NAME as u32) as usize;
            let name_bytes = &NAMES[i as usize][..name_len];
            if let Ok(name) = core::str::from_utf8(name_bytes) {
                let col = if is_sel { COL_SELECTED_TEXT } else { COL_TEXT };
                text(name, 40, y, col);
            }
        }

        if count == 0 {
            text("No games found", 92, 120, COL_TEXT);
        }
    }

    // Footer
    let f = frame_count();
    if (f / 30) % 2 == 0 {
        text("Press A to play", 92, 215, COL_TEXT);
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
