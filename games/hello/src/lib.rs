#![no_std]

use handheld_sdk::*;

#[unsafe(no_mangle)]
pub extern "C" fn init() {
    // Set up a basic palette
    set_palette(0, 0, 0, 0); // Black background
    set_palette(1, 255, 255, 255); // White
    set_palette(2, 0, 200, 80); // Green
    set_palette(3, 80, 120, 255); // Blue
    set_palette(4, 255, 80, 80); // Red
    set_palette(5, 255, 200, 0); // Yellow
}

#[unsafe(no_mangle)]
pub extern "C" fn update() {}

#[unsafe(no_mangle)]
pub extern "C" fn draw() {
    clear(0);

    // Title text
    text("HANDHELD OS", 110, 40, 1);
    text("VexiiRiscv + k23", 92, 56, 2);

    // Draw some shapes to show off the graphics primitives
    rect_fill(60, 90, 40, 40, 3);
    rect(60, 90, 40, 40, 1);

    circle_fill(160, 110, 18, 4);
    circle(160, 110, 18, 1);

    // Triangle using lines
    line(220, 130, 240, 90, 5);
    line(240, 90, 260, 130, 5);
    line(260, 130, 220, 130, 5);

    text("Running on k23!", 92, 210, 1);
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
