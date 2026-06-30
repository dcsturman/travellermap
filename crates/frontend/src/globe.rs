//! Client-side WebGL spinning-globe renderer (callisto, dev-only).
//!
//! Replaces worldgen's heavy, jerky APNG globe (`projection=globe`) with a smooth
//! 60fps render: fetch one equirectangular texture (`&format=texture`) once, upload
//! it to a WebGL texture, and animate the rotation in a fragment shader — the GPU
//! does the cheap per-pixel orthographic warp while the expensive map generation
//! stays on (and is cached by) the worldgen server. The fragment shader is a port
//! of worldgen's `warp_into` (`../worldgen/src/worldmap/globe.rs`).
//!
//! [`start`] sets up the context + texture + rAF loop and returns a [`GlobeAnim`]
//! whose `Drop`/[`GlobeAnim::stop`] halts the loop and releases the closure — the
//! caller keeps it alive while the globe popup is open and drops it on close.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{
    HtmlCanvasElement, HtmlImageElement, WebGlProgram, WebGlRenderingContext as Gl, WebGlShader,
    WebGlUniformLocation,
};

const VERT: &str = r"
attribute vec2 position;
varying vec2 p;
void main() {
  p = position;            // [-1,1], y up
  gl_Position = vec4(position, 0.0, 1.0);
}
";

// Verbatim port of worldgen's globe `warp_into` fragment shader.
const FRAG: &str = r"
precision highp float;
varying vec2 p;              // [-1,1], y up
uniform sampler2D uTex;
uniform float uSpin;
uniform vec2  uBeacon;       // starport (lon, lat) radians
uniform float uHasBeacon;    // 1.0 if a beacon exists, else 0.0

const float PI       = 3.14159265359;
const float TILT     = 0.41;                 // axial tilt
const float DISC     = 0.92;                 // disc radius within [-1,1]
const float GLOW     = 0.06;                 // atmosphere ring thickness
const vec3  ATMO     = vec3(130.,175.,255.)/255.;
const vec3  SUN      = normalize(vec3(-0.5,-0.3,0.6)); // fixed in view space
const float NIGHT    = 0.18;                 // night-side brightness
const float TERM     = 0.18;                 // terminator softness
const float LIMB     = 0.74;                 // limb-darkening floor
const vec3  CITY     = vec3(255.,185.,110.)/255.;
const float CITY_GAIN= 0.95;
const vec3  BEACONC  = vec3(255.,45.,35.)/255.;
const float BEACON_R = 0.07;                 // beacon angular radius (rad)
const float PULSES   = 8.0;

void main() {
  float dist = length(p);
  float glowOuter = DISC * (1.0 + GLOW);
  if (dist > glowOuter) { discard; }
  if (dist > DISC) {                          // atmosphere glow ring
    float tt = clamp((glowOuter - dist)/(glowOuter - DISC), 0.0, 1.0);
    gl_FragColor = vec4(ATMO, tt*tt*0.59);
    return;
  }
  // inverse orthographic -> near-hemisphere normal
  float nx = p.x/DISC, ny = p.y/DISC;
  float nz = sqrt(max(0.0, 1.0 - nx*nx - ny*ny));
  vec3 n = vec3(nx, ny, nz);
  // planet basis in view space (pole tilted toward viewer)
  float ca = cos(TILT), sa = sin(TILT);
  vec3 east=vec3(1.,0.,0.), north=vec3(0.,ca,sa), front=vec3(0.,-sa,ca);
  float lat = asin(clamp(dot(n,north),-1.,1.));
  float lon = atan(dot(n,east), dot(n,front)) + uSpin;
  vec2 uv = vec2(fract(lon/(2.0*PI)), (PI*0.5 - lat)/PI);
  vec4 t = texture2D(uTex, uv);
  vec3 col = t.rgb;
  // day/night terminator + limb darkening
  float day = smoothstep(-TERM, TERM, dot(n, SUN));
  float shade = (NIGHT + (1.0-NIGHT)*day) * (LIMB + (1.0-LIMB)*nz);
  col *= shade;
  // night-side city lights (emissive in alpha)
  col += CITY * (t.a * (1.0-day) * CITY_GAIN);
  // bright atmosphere rim on the lit limb
  float edge = dist/DISC;
  if (edge > 0.92) col = mix(col, ATMO, clamp((edge-0.92)/0.08,0.,1.)*0.5*day);
  // pulsing red starport beacon (day + night)
  if (uHasBeacon > 0.5) {
    vec3 s  = vec3(cos(lat)*cos(lon), cos(lat)*sin(lon), sin(lat));
    vec3 bd = vec3(cos(uBeacon.y)*cos(uBeacon.x), cos(uBeacon.y)*sin(uBeacon.x), sin(uBeacon.y));
    float b = smoothstep(cos(BEACON_R), 1.0, dot(s, bd)); b *= b;
    float pulse = 0.65 + 0.35*(0.5 + 0.5*sin(uSpin*PULSES));
    col = mix(col, BEACONC, b*pulse);
  }
  gl_FragColor = vec4(col, 1.0);
}
";

/// Spin rate in radians/second (worldgen's `~10.5°/s` to taste).
const SPIN_RATE: f64 = 0.6;

/// The self-rescheduling rAF callback, owned so the loop can re-borrow it to
/// re-arm each frame and dropped to end the loop.
type Tick = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;

fn compile(gl: &Gl, kind: u32, src: &str) -> Option<WebGlShader> {
    let sh = gl.create_shader(kind)?;
    gl.shader_source(&sh, src);
    gl.compile_shader(&sh);
    if gl
        .get_shader_parameter(&sh, Gl::COMPILE_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Some(sh)
    } else {
        None
    }
}

fn link(gl: &Gl, vert: &WebGlShader, frag: &WebGlShader) -> Option<WebGlProgram> {
    let prog = gl.create_program()?;
    gl.attach_shader(&prog, vert);
    gl.attach_shader(&prog, frag);
    gl.link_program(&prog);
    if gl
        .get_program_parameter(&prog, Gl::LINK_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Some(prog)
    } else {
        None
    }
}

/// Compile + link the globe program on `gl`, upload `tex_img` as the equirect
/// texture, and wire the static uniforms (texture unit + beacon). Returns the
/// `uSpin` location for the caller to drive per frame. Shared by the live
/// [`start`] rAF loop and the offscreen [`capture_frames`] GIF path. `None` on
/// any GL failure. Leaves blending enabled; the caller sets the viewport.
fn setup(
    gl: &Gl,
    tex_img: &HtmlImageElement,
    starport: Option<(f32, f32)>,
) -> Option<WebGlUniformLocation> {
    let vert = compile(gl, Gl::VERTEX_SHADER, VERT)?;
    let frag = compile(gl, Gl::FRAGMENT_SHADER, FRAG)?;
    let prog = link(gl, &vert, &frag)?;
    gl.use_program(Some(&prog));

    // Fullscreen quad ([-1,1]²) as a triangle strip; `p` = position in the shader.
    let buf = gl.create_buffer()?;
    gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&buf));
    let verts: [f32; 8] = [-1., -1., 1., -1., -1., 1., 1., 1.];
    let arr = js_sys::Float32Array::new_with_length(8);
    arr.copy_from(&verts);
    gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &arr, Gl::STATIC_DRAW);
    let pos = gl.get_attrib_location(&prog, "position") as u32;
    gl.vertex_attrib_pointer_with_i32(pos, 2, Gl::FLOAT, false, 0, 0);
    gl.enable_vertex_attrib_array(pos);

    // Texture: REPEAT in longitude, CLAMP in latitude, LINEAR, straight alpha
    // (alpha is city-light data, not opacity), no flip / colorspace conversion.
    let tex = gl.create_texture()?;
    gl.bind_texture(Gl::TEXTURE_2D, Some(&tex));
    gl.pixel_storei(Gl::UNPACK_FLIP_Y_WEBGL, 0);
    gl.pixel_storei(Gl::UNPACK_PREMULTIPLY_ALPHA_WEBGL, 0);
    gl.pixel_storei(Gl::UNPACK_COLORSPACE_CONVERSION_WEBGL, Gl::NONE as i32);
    gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_S, Gl::REPEAT as i32);
    gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_T, Gl::CLAMP_TO_EDGE as i32);
    gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_MIN_FILTER, Gl::LINEAR as i32);
    gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_MAG_FILTER, Gl::LINEAR as i32);
    gl.tex_image_2d_with_u32_and_u32_and_image(
        Gl::TEXTURE_2D,
        0,
        Gl::RGBA as i32,
        Gl::RGBA,
        Gl::UNSIGNED_BYTE,
        tex_img,
    )
    .ok()?;

    // Static uniforms (only uSpin changes per frame).
    let u_tex = gl.get_uniform_location(&prog, "uTex");
    let u_spin = gl.get_uniform_location(&prog, "uSpin")?;
    let u_beacon = gl.get_uniform_location(&prog, "uBeacon");
    let u_has = gl.get_uniform_location(&prog, "uHasBeacon");
    gl.active_texture(Gl::TEXTURE0);
    gl.uniform1i(u_tex.as_ref(), 0);
    let (blon, blat) = starport.unwrap_or((0.0, 0.0));
    gl.uniform2f(u_beacon.as_ref(), blon, blat);
    gl.uniform1f(u_has.as_ref(), if starport.is_some() { 1.0 } else { 0.0 });
    gl.enable(Gl::BLEND);
    gl.blend_func(Gl::SRC_ALPHA, Gl::ONE_MINUS_SRC_ALPHA);
    Some(u_spin)
}

/// Whether the browser can give us a WebGL context — checked before choosing the
/// WebGL globe over the APNG fallback.
pub fn webgl_available() -> bool {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return false;
    };
    let Some(canvas) = doc
        .create_element("canvas")
        .ok()
        .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok())
    else {
        return false;
    };
    matches!(canvas.get_context("webgl"), Ok(Some(_)))
}

/// A running globe animation: holds the live rAF closure + cancellation state.
/// Dropping it (or calling [`GlobeAnim::stop`]) ends the loop and frees the GPU
/// resources via the dropped closure.
pub struct GlobeAnim {
    alive: Rc<Cell<bool>>,
    raf: Rc<Cell<i32>>,
    // Owns the self-rescheduling rAF closure for the loop's lifetime; cleared on
    // stop to break the closure↔tick Rc cycle (the closure holds a `tick` clone).
    tick: Tick,
}

impl GlobeAnim {
    pub fn stop(&self) {
        self.alive.set(false);
        let h = self.raf.get();
        if h != 0 {
            if let Some(w) = web_sys::window() {
                let _ = w.cancel_animation_frame(h);
            }
            self.raf.set(0);
        }
        // Drop the closure so the cycle (closure → tick → closure) is broken and it
        // is actually freed. Safe: stop() never runs while the callback is borrowing.
        if let Ok(mut t) = self.tick.try_borrow_mut() {
            t.take();
        }
    }
}

impl Drop for GlobeAnim {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Upload `tex_img` (a decoded equirectangular PNG: RGB = day surface, A = city
/// lights) to a WebGL texture on `canvas` and start a `requestAnimationFrame` loop
/// that spins the globe by driving `uSpin` from wall-clock time (frame-rate
/// independent). `starport` is the beacon's `(lon, lat)` in radians, or `None`.
/// Returns `None` if any GL setup step fails (caller falls back to the APNG).
pub fn start(
    canvas: &HtmlCanvasElement,
    tex_img: &HtmlImageElement,
    starport: Option<(f32, f32)>,
) -> Option<GlobeAnim> {
    let win = web_sys::window()?;
    let dpr = win.device_pixel_ratio().max(1.0);
    // Size the drawing buffer to the CSS box × DPR (square canvas).
    let css = f64::from(canvas.client_width().max(1));
    let px = (css * dpr).round().max(1.0) as u32;
    canvas.set_width(px);
    canvas.set_height(px);

    let gl: Gl = canvas.get_context("webgl").ok()??.dyn_into().ok()?;
    let u_spin = setup(&gl, tex_img, starport)?;
    gl.viewport(0, 0, px as i32, px as i32);

    // Self-rescheduling rAF loop, halted by `alive`/`stop()`.
    let alive = Rc::new(Cell::new(true));
    let raf = Rc::new(Cell::new(0));
    let tick: Tick = Rc::new(RefCell::new(None));
    let draw = draw_closure(
        gl,
        u_spin,
        px as i32,
        alive.clone(),
        raf.clone(),
        tick.clone(),
        win.clone(),
    );
    let h = win
        .request_animation_frame(draw.as_ref().unchecked_ref())
        .ok()?;
    raf.set(h);
    *tick.borrow_mut() = Some(draw);
    Some(GlobeAnim { alive, raf, tick })
}

/// Build the per-frame closure: clear, set `uSpin` from the rAF timestamp, draw,
/// then reschedule itself while `alive`.
fn draw_closure(
    gl: Gl,
    u_spin: WebGlUniformLocation,
    size: i32,
    alive: Rc<Cell<bool>>,
    raf: Rc<Cell<i32>>,
    tick: Tick,
    win: web_sys::Window,
) -> Closure<dyn FnMut(f64)> {
    Closure::wrap(Box::new(move |ts: f64| {
        if !alive.get() {
            return;
        }
        gl.viewport(0, 0, size, size);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(Gl::COLOR_BUFFER_BIT);
        let spin = (ts / 1000.0) * SPIN_RATE;
        gl.uniform1f(Some(&u_spin), spin as f32);
        gl.draw_arrays(Gl::TRIANGLE_STRIP, 0, 4);
        // Reschedule from the closure `tick` owns (an immutable re-borrow is safe:
        // we're inside the call, nothing holds a mutable borrow).
        if let Some(cb) = tick.borrow().as_ref() {
            let h = win
                .request_animation_frame(cb.as_ref().unchecked_ref())
                .unwrap_or(0);
            raf.set(h);
        }
    }) as Box<dyn FnMut(f64)>)
}

/// Parse an `X-Starport: <lon>,<lat>` header value (radians) → `(lon, lat)`.
pub fn parse_starport(header: &str) -> Option<(f32, f32)> {
    let (lon, lat) = header.split_once(',')?;
    Some((
        lon.trim().parse::<f32>().ok()?,
        lat.trim().parse::<f32>().ok()?,
    ))
}

/// Helper so `main` can build the texture endpoint from a base `/api/world` URL.
pub fn texture_url(base: &str) -> String {
    format!("{base}&projection=globe&format=texture")
}

/// Render `frames` evenly-spaced rotation steps of the globe into an offscreen
/// `size`×`size` WebGL canvas and read each one back as RGBA. Spin angles are
/// `2π·i/frames`, so the sequence is exactly periodic → a seamless GIF loop.
/// Frames come back **top-down** (rows already flipped from WebGL's bottom-up
/// origin). `None` on any GL failure. Used to share the spinning globe to Discord.
#[cfg(feature = "callisto")]
pub fn capture_frames(
    tex_img: &HtmlImageElement,
    starport: Option<(f32, f32)>,
    size: u32,
    frames: u32,
) -> Option<Vec<Vec<u8>>> {
    use core::f32::consts::PI;

    let doc = web_sys::window()?.document()?;
    let canvas: HtmlCanvasElement = doc.create_element("canvas").ok()?.dyn_into().ok()?;
    canvas.set_width(size);
    canvas.set_height(size);

    // Preserve the drawing buffer so read_pixels is robust across drivers (we read
    // synchronously after each draw, but this removes any compositing-timing doubt).
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"preserveDrawingBuffer".into(), &true.into());
    let _ = js_sys::Reflect::set(&opts, &"premultipliedAlpha".into(), &false.into());
    let gl: Gl = canvas
        .get_context_with_context_options("webgl", &opts)
        .ok()??
        .dyn_into()
        .ok()?;

    let u_spin = setup(&gl, tex_img, starport)?;
    let s = size as i32;
    gl.viewport(0, 0, s, s);

    let stride = size as usize * 4;
    let mut buf = vec![0u8; stride * size as usize];
    (0..frames)
        .map(|i| {
            let spin = 2.0 * PI * (i as f32) / (frames as f32);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(Gl::COLOR_BUFFER_BIT);
            gl.uniform1f(Some(&u_spin), spin);
            gl.draw_arrays(Gl::TRIANGLE_STRIP, 0, 4);
            gl.read_pixels_with_opt_u8_array(
                0,
                0,
                s,
                s,
                Gl::RGBA,
                Gl::UNSIGNED_BYTE,
                Some(&mut buf),
            )
            .ok()?;
            // WebGL reads bottom-up; reverse rows for a top-down GIF frame.
            Some(buf.chunks_exact(stride).rev().flatten().copied().collect())
        })
        .collect()
}

/// Encode RGBA `frames` (top-down, `size`×`size`) into a single infinitely-looping
/// animated GIF. Because every frame is the same texture merely rotated, one global
/// 256-colour palette (NeuQuant on a representative frame) is near-lossless and
/// keeps the file small. `delay_cs` is the per-frame delay in centiseconds. The
/// globe renders on a transparent clear, so each frame is flattened over the popup's
/// dark backdrop first. `None` on encode failure.
#[cfg(feature = "callisto")]
pub fn frames_to_gif(frames: &[Vec<u8>], size: u16, delay_cs: u16) -> Option<Vec<u8>> {
    use color_quant::NeuQuant;
    use gif::{Encoder, Frame, Repeat};

    const BG: [u8; 3] = [0x0b, 0x0e, 0x16]; // popup backdrop (#0b0e16)

    // RGBA-over-BG → opaque RGBA (alpha forced to 255 so NeuQuant + index_of agree).
    let flatten = |rgba: &[u8]| -> Vec<u8> {
        rgba.chunks_exact(4)
            .flat_map(|px| {
                let a = u32::from(px[3]);
                let mix =
                    |c: u8, b: u8| ((u32::from(c) * a + u32::from(b) * (255 - a)) / 255) as u8;
                [mix(px[0], BG[0]), mix(px[1], BG[1]), mix(px[2], BG[2]), 255]
            })
            .collect::<Vec<u8>>()
    };
    let flat: Vec<Vec<u8>> = frames.iter().map(|f| flatten(f)).collect();
    if flat.is_empty() {
        return None;
    }

    // One shared palette, trained on frames spread across the whole rotation (not
    // a single mid-frame) and at the finest sample factor. Small, intermittent
    // accents — the rotating red starport beacon and the sparse night-side city
    // lights — occupy too few pixels to survive a coarse single-frame palette;
    // they'd map to the nearest dark surface colour and vanish. Sampling several
    // rotation phases ensures the beacon (facing the viewer in some frames) and
    // the city specks get their own palette entries.
    let mut training: Vec<u8> = Vec::new();
    let step = (flat.len() / 8).max(1);
    for f in flat.iter().step_by(step) {
        training.extend_from_slice(f);
    }
    let nq = NeuQuant::new(1, 256, &training);
    let palette = nq.color_map_rgb();

    let mut out: Vec<u8> = Vec::new();
    {
        let mut enc = Encoder::new(&mut out, size, size, &palette).ok()?;
        enc.set_repeat(Repeat::Infinite).ok()?;
        for f in &flat {
            let indices: Vec<u8> = f.chunks_exact(4).map(|px| nq.index_of(px) as u8).collect();
            let frame = Frame {
                width: size,
                height: size,
                buffer: std::borrow::Cow::Owned(indices),
                delay: delay_cs,
                ..Frame::default()
            };
            enc.write_frame(&frame).ok()?;
        }
    }
    Some(out)
}
