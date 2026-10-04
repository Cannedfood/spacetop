# Technical Documentation

## Library And Executable

The Cargo package provides both the `spacetop` library and executable:

- [src/lib.rs](src/lib.rs) owns private compositor state and Wayland protocol
	handlers. Rendering, input, X11, and XR implementation modules remain private.
- [src/runtime.rs](src/runtime.rs) owns backend startup, sockets, the event loop,
	and fatal-error propagation. The library exports `run`, `run_xr_client`, and
	`DisplayNames` from this module.
- [src/main.rs](src/main.rs) handles CLI parsing and application process spawning,
	including the child's display environment. The library does not parse CLI
	arguments or choose which application to launch.

`run` blocks until the compositor stops and accepts a one-shot readiness callback
receiving `DisplayNames`: the Wayland socket name and an optional X11 display.
The callback runs on the compositor thread after XWayland's window manager is
ready, or after socket registration when XWayland is unavailable. Readiness does
not imply that XR/GPU setup has completed. Callback failures stop the compositor.
The executable uses this callback for `--app`; library callers can provide their
own startup action. `run_xr_client` retains the standalone XR mode.

## X11 Applications

Spacetop starts a private rootless XWayland server when `Xwayland` is on `PATH`.
XWayland 23.1 or newer is required for the surface-association protocol.
Smithay's X11 window manager handles mapping, application-requested resizing,
unmapping, remapping, destruction, activation, and ICCCM keyboard focus.
Associated Wayland surfaces use the same GPU capture, XR panels, and controller
pointer input as native Wayland windows. X11 override-redirect menus/tooltips
are composed into their managed parent's panel at their X11-relative position.
Transient-for hints select the parent (including nested menus); the active X11
window or an overlapping managed window is used as a fallback. An unowned
override-redirect window still gets its own panel. Keyboard focus stays on the
managed parent. Full X11 stacking and window-type policy remain follow-ups.
Clipboard exchange between X11 and Wayland, window decorations, and interactive
window movement/resizing are not implemented.

`--app=PROGRAM` waits until the X11 window manager is ready and supplies both
`WAYLAND_DISPLAY` and `DISPLAY`. The compositor prints those display names for
manual launches. If XWayland is not installed, startup continues in Wayland-only
mode, and `--app` removes the inherited `DISPLAY` so X11 apps cannot accidentally
open on the host desktop. Other XWayland startup or connection failures are fatal.

## Menus And Input

Native xdg popups use Smithay's popup manager, positioner geometry, nested grabs,
serial/seat/client validation, reposition configures, and outside-client click
dismissal. Popup and subsurface commits refresh the owning panel. Menus are
drawn above the window into the same image, including content outside its buffer
bounds. Capture bounds expand without moving the parent's content or changing
its physical pixel scale, and shrink again on dismissal/destruction. Popup
placement is unconstrained in the spatial plane; finite desktop work-area
constraints and reactive reconfiguration are not implemented.

Pointer targeting follows surface stacking and input regions with per-surface
coordinate offsets. Client window geometry is included when positioning popups.
Client-requested cursor images are not rendered yet; the targeting cross remains.
Controller secondary click opens context menus on supported profiles. Button
transitions use reliable channel sends, with the current ray sent first, and
tracking/session loss releases held XR buttons and clears pointer focus. These
sends use an unbounded reliable channel and do not wait for compositor queue
capacity. Best-effort motion, scroll, and spatial movement are rejected once
16 events are pending. Reliable events bypass that limit and retain FIFO order,
including the ray preceding a button. Their backlog can grow during a prolonged
compositor stall; frame requests are separately coalesced to one queued tick.

The evdev backend automatically enumerates udev input event devices classified
as keyboards or mice on `XDG_SEAT` (default `seat0`). Setting the colon-separated
`SPACETOP_INPUT_DEVICES` overrides discovery; an empty value disables physical
input. Explicit paths must be accessible at startup. A one-second timer on the
compositor event loop reconciles devices, retries failed opens (including
permission changes), and reopens reconnected devices. Device-node identity
deduplicates aliases and detects replacement at the same path. Unavailable
devices are reported once per distinct error until they reconnect or leave the selection.
Keyboard events feed Smithay/XKB directly,
with modifier updates and client-side repeat (200 ms delay, 25 Hz). Kernel
repeat events are ignored to avoid double repeat. Evdev recovers dropped kernel
events; disconnect releases that device's held keys/buttons. Shared counts keep
keys/buttons held until all contributing devices release them. Mouse devices
provide left/right/middle/extra buttons and vertical/horizontal wheel events,
including value120 information, at the XR pointer location.

Input-device permissions must be configured externally; no exclusive grabs or
session access management are performed. The host desktop can receive the same input.
XKB uses `XKB_DEFAULT_*` environment settings. Text-input/IME, a virtual keyboard,
physical mouse-motion mapping, and relative-pointer/constraints remain
unsupported. See [README.md](README.md) for a launch example.

When no pointer grab is active, windows receive activation and keyboard focus
before their first pointer press. Existing implicit pointer grabs and popup
dismissal retain their focus routing.

## Clipboard And GTK Startup

Smithay's `DataDeviceState` advertises `wl_data_device_manager` and implements
native Wayland clipboard offers, MIME negotiation, and FD-based transfers.
Selection delivery follows actual keyboard focus, including popup focus and
focus loss/restoration. GTK requires this global during display initialization;
without it, clients can refuse to open the display before creating a window.

Client-initiated drag-and-drop uses Smithay's protocol/grab implementation, but
drag icons are not rendered and XR cross-panel interactions need real-toolkit
verification. Primary selection and X11/Wayland clipboard exchange are not
implemented.

## GPU Rendering

GPU sharing is mandatory and uses the OpenXR runtime's Vulkan GPU.
Smithay composites app buffers using GLES into linear RGBA DMA-BUF images.
The application's Vulkan renderer imports and samples those images directly,
without panel copies or CPU readback. It draws textured spatial panels and the
targeting cross into two eye-resolution OpenXR swapchains and submits one stereo
projection layer. OpenXR handles final headset composition, not window quads.
Each eye uses the runtime's recommended dimensions, predicted pose and asymmetric
FOV. The scene uses depth testing, back-to-front premultiplied-alpha blending,
and transparent-fragment rejection; the cursor has a minimum screen-space stroke.
WGSL shaders are compiled to SPIR-V with Naga.

The floor occupies a 60-meter square and uses the configured albedo and
transparency, with sky and window reflections rendered in one full-screen
environment pass. When OpenXR STAGE space is supported, its floor origin
(STAGE Y = 0) is located relative to LOCAL space at each frame's predicted
display time; that position's Y coordinate sets the rendered floor height.
Panels, views, and input remain in LOCAL space, preserving window placement.
The last valid calibrated height is retained during tracking loss. Before a
valid STAGE position is available, or if STAGE cannot be created, the fallback is
LOCAL-space Y = -1.3 meters. The environment pass generates each floor reflection
ray once and intersects it against the runtime window list, selecting the nearest
front-facing quad. By default a hit samples only that nearest window texture;
a miss samples the skybox. The optional
`floor.trace_through_transparent_windows` setting
continues through transparent texels and composites farther hits. Separate
precompiled shader pipeline variants keep the default first-hit path free of a
runtime mode branch. This avoids rendering a separate floor-reflection pass for
every window and, in the default mode, limits each ray to one selected window
texture. Rays follow an isotropic
GGX visible-normal distribution: the shader
samples view-visible microfacet normals and reflects the eye direction about
each one. Analytic ray/quad intersections provide window texture coordinates.
The configured skybox exposure and rotation apply to reflected sky samples;
roughness broadens them with a five-tap filter.
Schlick Fresnel and height-correlated Smith masking provide the BRDF/PDF weight
`F * G2 / G1(view)`. There is no extra area or inverse-square multiplier: distance
changes the window's angular coverage instead. Premultiplied texture colors
include transparency, and windows emit from their front side only.

Material and sampling constants are at the top of the WGSL shader:
`FLOOR_ROUGHNESS` defaults to 0.25 (GGX alpha is roughness squared),
`FLOOR_REFLECTANCE` defaults to 0.18 and denotes normal-incidence Fresnel
reflectance, and `FLOOR_RAY_COUNT` defaults to four. Lower roughness concentrates
rays near the mirror direction; higher roughness broadens the reflection.
Window quads occlude farther windows along each reflection ray; there are no
shadow queries or ray-tracing extensions. Each ray gets an independent
pseudorandom jitter inside its equal-area
sample stratum, seeded by floor-world X/Z coordinates rounded to cells sized by
`floor.reflection_grain_size_m` (default 2 mm, range 1-50 mm) and ray index with
an integer PCG hash. Signed coordinates remain distinct, and height recalibration
does not reseed the pattern. Seeds follow the floor rather than framebuffer pixels or headset motion;
GGX reflection directions still respond to the eye position. Four samples remain
noisy; increasing the ray count improves coverage.
There is no temporal accumulation or denoising.

GPU-backed Wayland client buffers are supported through `linux-dmabuf` with
device feedback. Shared-memory client buffers still require an upload to GLES.
The GLES renderer and Vulkan session use the same DRM render node; the XR
runtime itself does not need to support GLES.

The renderer requires Vulkan 1.2, including runtime descriptor arrays,
non-uniform sampled-image-array indexing, and variable descriptor counts. It
queries and enables those optional Vulkan 1.2 features at startup and fails with
a clear error when the runtime-selected device lacks them. Reflected window
count is limited by the selected GPU's sampled-image and storage-buffer limits.
It also requires the external-memory FD,
DMA-BUF, DRM-format-modifier, physical-device-DRM, image-format-list, and
foreign-queue-family device extensions. It also needs a driver that supports
linear RGBA8 GLES render targets and sampled sRGB Vulkan DMA-BUF imports, an sRGB
OpenXR swapchain, D32 depth attachments, and access to the runtime GPU's
`/dev/dri/renderD*` node.
Build dependencies include EGL, GLES, DRM, GBM (with modifier-aware allocation
and per-plane FD export), and xkbcommon development libraries.

Startup logs report the selected render node. Missing GPU capabilities,
renderer initialization failures, panel capture failures, and Vulkan DMA-BUF
import failures stop the compositor with an error. There is no CPU renderer,
panel readback/upload fallback, or renderer-selection environment variable.
App redraws wait for GPU setup to complete.

Surface commits update logical geometry and mark only their owning panel dirty;
they do not allocate or render a panel image. The XR thread requests a capture
tick when `wait_frame` allows rendering. After event dispatch, the compositor
captures each dirty panel at most once for that tick, using the latest committed
state, and then completes its frame callbacks. Clean panels are not recaptured.
Popup clicks invalidate popup owners, not every window. Unmap/close updates are
published immediately. No captures or frame callbacks are scheduled while XR
is not requesting renderable frames; this is cadence control, not proof of
presentation or per-panel visibility tracking.

Each mapped toplevel gets an independent sampled texture and spatial panel. Windows occupy
stable, nonoverlapping positions, starting in the center and alternating right
and left. Commits and resizing do not move other windows; closed-window positions
can be reused. Unmapping or closing removes the panel, and remapping restores it.
The targeting cross and pointer select the nearest intersected window, and
clicking transfers keyboard focus. Closing or unmapping the active window hands
focus to another mapped window.

Images use the window's logical size and buffer scale, rather than a fixed
512-pixel texture. Large windows are scaled proportionally to fit the Vulkan
device limit and a default 4096-pixel per-dimension
cap. Set `SPACETOP_MAX_PANEL_SIZE` to a positive integer to change that cap.
Startup logs report the negotiated panel image size. Window count is no longer
constrained by the runtime's composition-layer limit; all panels and the cursor
share one projection layer.

This is not yet a fully pipelined renderer: each scheduled capture allocates a fresh
shared image, producer completion is waited on by the CPU, and Vulkan waits for
scene rendering before releasing eye swapchain images. Clean panels retain their
sampled textures, but the stereo scene is redrawn each renderable frame. Foreign
queue-family ownership is acquired before sampling and released afterward.
Invalid view tracking submits no layers. Commits coalesce
before capture, and pending images coalesce per window without dropping
unmap/close notifications. Buffer pooling and
explicit GPU semaphore handoff remain follow-ups. Grip-driven spatial movement
is supported; client-requested movement and logical resizing controls are not.

## Timing Diagnostics

[src/timing.rs](src/timing.rs) measures wall-clock time using monotonic `Instant`
timestamps. Warnings have a default 8 ms minimum, configurable through the
positive-integer `SPACETOP_SLOW_MS` environment variable. Invalid or zero values
fall back to 8 ms. Each stage logs at most once per second, retaining suppressed
slow-event counts and the worst duration since its previous report. Fast paths
do not log or create per-stage report entries.

The scopes distinguish where delays are observed:

- `app/wayland-dispatch`, `app/wayland-flush`, and `app/input-dispatch` measure
	compositor event processing. `app/compositor-capture` includes the whole
	capture pipeline, including GLES completion; `gpu/gles-completion` isolates
	the explicit GPU wait. `mixed/compositor-batch` catches cumulative capture
	costs across multiple windows.
- `app/panel-updates` and `app/xr-frame-work` exclude measured OpenXR/GPU calls
	and resource retirement. The latter uses one predicted frame period as its
	budget. GPU command recording and other unwrapped driver work remain in
	this application scope; it is elapsed time, not CPU utilization.
- `gpu/...` times DMA-BUF import/replacement, Vulkan submission, and fence waits.
	`gpu/frame-work` also checks their aggregate time against one frame period.
- `openxr/...` times polling, session/swapchain operations, tracking/action calls,
	frame begin/end, and image acquire/wait/release. `openxr/frame-calls` aggregates
	calls during active frame work, excluding `wait_frame` and the final `end_frame`.
- `mixed/xr-active-frame` checks total work after `wait_frame` and before
	`end_frame`, then prints the current frame's app/OpenXR/GPU breakdown. It can
	report a missed budget even when each domain is individually below its limit.
	Resource retirement is also labeled `mixed` because destructors can invoke
	both OpenXR and Vulkan.

Normal pacing in `wait_frame` is expected: warnings require more than three
predicted frame periods, and waits for non-renderable frames are not reported.
`begin_frame` and image acquisition allow one period; image waiting and
`end_frame` allow two. Session begin/end allow 250 ms. The configured minimum
applies to all these limits. Individual and aggregate scopes overlap and must
not be added together as independent costs.

These are client-side boundary measurements, not OpenXR server profiling.
Long runtime calls may reflect server scheduling, IPC, shared-GPU contention,
driver work, or OS descheduling. Initial shader compilation and window creation
can legitimately produce isolated warnings; recurring warnings are more useful
for sustained performance problems. Timers report when calls return, not while
a call is indefinitely hung. No diagnostic code connects to a second runtime
or changes synchronization behavior.

## Tests

Application tests live under `tests/` and are included with test-only `#[path]`
modules, preserving access to private internals without adding production APIs.
Compositor and backend tests belong to the library target; CLI tests belong to
the executable target. `cargo test --lib` and `cargo test --bin spacetop` can run
those targets separately.
These are unit-test modules, not separate Cargo integration-test binaries:

- [tests/compositor/mod.rs](tests/compositor/mod.rs) groups the Wayland and X11
	scenarios, shared protocol client, and compositor failure tests. Wayland checks
	are split into basic rendering/input, menus, subsurfaces, and multiwindow
	scenarios with one shared fixture.
- [tests/unit/panel.rs](tests/unit/panel.rs) and neighboring unit modules cover
	geometry, input state, channels, CLI arguments, and XR color formats.
- [tests/support/gpu.rs](tests/support/gpu.rs) contains test-only Vulkan setup,
	pixel readback, and cursor checks.

Existing test-name filters and opt-in hardware requirements are unchanged.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

An opt-in real-GTK startup regression launches Zenity on a private Wayland
socket with its Cairo renderer, independently of XR/GPU startup:

```sh
cargo test gtk_app_maps_a_window -- --ignored --nocapture
```

This requires `zenity` on `PATH`. Default protocol tests also check clipboard
MIME offers/text transfer/focus restoration and focus-before-click ordering.
Additional regressions check input saturation/order, coalesced frame requests,
and popup-owner-only invalidation. Protocol fixtures simulate frame ticks
without starting an OpenXR runtime. The GPU scheduling regression verifies
that ten rapid commits produce no image before a tick, one image at the tick,
and no new image on a clean tick:

```sh
cargo test gpu_frame_ticks_coalesce_commits_and_skip_clean_panels -- --ignored --nocapture
```

An opt-in hardware test creates a real Wayland app, composites through GLES,
imports into Vulkan, checks image-copy and cursor pixels, exercises GPU-backed
client buffers, and verifies deferred GPU setup, multiwindow focus and nearest
hits, clicks, redraws, 1600x800 capture, resize, unmap/remap, close, placement reuse,
update coalescing, and layer-limit enforcement. It also verifies popup pixels
outside the parent, nested/repositioned menus, invalid-grab dismissal,
subsurface/input-region hit-testing, keyboard modifiers, and tracking-loss release.
Test-only readback verifies pixels; the running compositor never reads panel
images back to the CPU:

```sh
cargo test gpu_shared_app -- --ignored --nocapture
```

To select a particular GPU for that test:

```sh
SPACETOP_GPU_TEST_NODE=/dev/dri/renderD129 cargo test gpu_shared_app -- --ignored --nocapture
```

Opt-in X11 tests start the real XWayland server. One checks deferred GPU setup,
pointer input, real key events, parent-relative override-redirect menu input,
keyboard focus, and close. The GPU-backed test also verifies menu pixels,
application-requested resize, unmap/remap, and placement reuse:

```sh
cargo test x11_app -- --ignored --nocapture --test-threads=1
```

These require `Xwayland` and a valid `XDG_RUNTIME_DIR`; the GPU-backed test also
requires a compatible render node and Vulkan DMA-BUF import support.
