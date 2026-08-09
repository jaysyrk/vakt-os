mod sandbox;

use memmap2::MmapMut;
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::time::Duration;

/// The device this program exists to draw on, and - once the sandbox is up -
/// the only file it can reach.
const FRAMEBUFFER: &str = "/dev/fb0";

/// sandbox::confine() can only grant a Landlock rule for a path that exists
/// at the moment the ruleset locks in, and the ruleset can never be widened
/// afterward - so if the framebuffer driver is a loadable module that hasn't
/// finished probing yet, confining before it appears would permanently deny
/// this process the one device it exists to draw on. A short wait here costs
/// nothing in the ordinary case (the device already exists, so the loop exits
/// on its first check).
fn wait_for_framebuffer() {
    const ATTEMPTS: u32 = 10;
    const DELAY: Duration = Duration::from_millis(100);
    for attempt in 0..ATTEMPTS {
        if Path::new(FRAMEBUFFER).exists() {
            return;
        }
        if attempt + 1 < ATTEMPTS {
            std::thread::sleep(DELAY);
        }
    }
}

const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

#[repr(C)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
struct FbVarScreenInfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
struct FbFixScreenInfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

fn main() {
    wait_for_framebuffer();

    match sandbox::confine(Path::new(FRAMEBUFFER)) {
        Ok(report) => println!("[Vakt-Compositor] {}", report),
        Err(e) => println!(
            "[Vakt-Compositor] Could not apply the Landlock sandbox: {}",
            e
        ),
    }

    let fb = OpenOptions::new()
        .read(true)
        .write(true)
        .open(FRAMEBUFFER)
        .unwrap_or_else(|e| panic!("Failed to open {}: {}", FRAMEBUFFER, e));

    let fd = fb.as_raw_fd();

    let mut vinfo = std::mem::MaybeUninit::<FbVarScreenInfo>::uninit();
    let mut finfo = std::mem::MaybeUninit::<FbFixScreenInfo>::uninit();

    unsafe {
        if libc::ioctl(fd, FBIOGET_FSCREENINFO, finfo.as_mut_ptr()) == -1 {
            panic!("Error reading fixed information");
        }
        if libc::ioctl(fd, FBIOGET_VSCREENINFO, vinfo.as_mut_ptr()) == -1 {
            panic!("Error reading variable information");
        }
    }

    let vinfo = unsafe { vinfo.assume_init() };
    let finfo = unsafe { finfo.assume_init() };

    let screensize = finfo.smem_len as usize;

    let mut map = unsafe { MmapMut::map_mut(&fb).expect("Failed to mmap framebuffer") };

    println!(
        "Resolution: {}x{}, {} bpp",
        vinfo.xres, vinfo.yres, vinfo.bits_per_pixel
    );

    // Draw a dark gray background
    for y in 0..vinfo.yres {
        for x in 0..vinfo.xres {
            let location = (x + vinfo.xoffset) * (vinfo.bits_per_pixel / 8)
                + (y + vinfo.yoffset) * finfo.line_length;

            let loc = location as usize;
            if loc + 3 < screensize && vinfo.bits_per_pixel == 32 {
                map[loc] = 30; // B
                map[loc + 1] = 30; // G
                map[loc + 2] = 30; // R
                map[loc + 3] = 0; // A
            }
        }
    }

    // Draw a blue square in the middle (Vakt OS Mock GUI). Skipped on a
    // display smaller than the square itself, rather than computing a
    // position from an underflowed subtraction.
    let square_size = 300;
    if vinfo.xres >= square_size && vinfo.yres >= square_size {
        let start_x = (vinfo.xres - square_size) / 2;
        let start_y = (vinfo.yres - square_size) / 2;

        for y in start_y..(start_y + square_size) {
            for x in start_x..(start_x + square_size) {
                let location = (x + vinfo.xoffset) * (vinfo.bits_per_pixel / 8)
                    + (y + vinfo.yoffset) * finfo.line_length;

                let loc = location as usize;
                if loc + 3 < screensize && vinfo.bits_per_pixel == 32 {
                    map[loc] = 255; // B
                    map[loc + 1] = 50; // G
                    map[loc + 2] = 50; // R
                    map[loc + 3] = 0; // A
                }
            }
        }
    }

    println!("[Vakt-Compositor] Rendered directly to /dev/fb0");
    std::thread::sleep(std::time::Duration::from_secs(10));
}
