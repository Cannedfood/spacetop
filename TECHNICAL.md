# Technical Documentation

## X11 Applications

Spacetop starts a private rootless XWayland server when `Xwayland` is on `PATH`.
XWayland 23.1 or newer is required for the surface-association protocol.
Smithay's X11 window manager handles mapping, application-requested resizing,
unmapping, remapping, destruction, activation, and ICCCM keyboard focus.
Associated Wayland surfaces use the same GPU capture, XR panels, and controller
pointer input as native Wayland windows. X11 override-redirect windows are
currently presented as independent panels, not positioned menus over their parent.
Clipboard exchange between X11 and Wayland, window decorations, and interactive
window movement/resizing are not implemented.

`--app=PROGRAM` waits until the X11 window manager is ready and supplies both
`WAYLAND_DISPLAY` and `DISPLAY`. The compositor prints those display names for
manual launches. If XWayland is not installed, startup continues in Wayland-only
mode, and `--app` removes the inherited `DISPLAY` so X11 apps cannot accidentally
open on the host desktop. Other XWayland startup or connection failures are fatal.

## GPU Rendering

GPU sharing is mandatory and uses the OpenXR runtime's Vulkan GPU.
Smithay composites app buffers using GLES into linear RGBA DMA-BUF images.
Vulkan imports those images and copies them into per-window OpenXR swapchains without
reading panel pixels back to the CPU. The targeting cross uses a tiny persistent
GPU buffer instead of modifying panel pixels.

GPU-backed Wayland client buffers are supported through `linux-dmabuf` with
device feedback. Shared-memory client buffers still require an upload to GLES.
The GLES renderer and Vulkan session use the same DRM render node; the XR
runtime itself does not need to support GLES.

This initial implementation requires Vulkan 1.1 and the external-memory FD,
DMA-BUF, DRM-format-modifier, physical-device-DRM, image-format-list, and
foreign-queue-family device extensions. It also needs a driver that supports
linear RGBA8 GLES render targets and sRGB Vulkan DMA-BUF imports, an sRGB OpenXR
swapchain, and access to the runtime GPU's `/dev/dri/renderD*` node.
Build dependencies include EGL, GLES, DRM, GBM (with modifier-aware allocation
and per-plane FD export), and xkbcommon development libraries.

Startup logs report the selected render node. Missing GPU capabilities,
renderer initialization failures, panel capture failures, and Vulkan DMA-BUF
import failures stop the compositor with an error. There is no CPU renderer,
panel readback/upload fallback, or renderer-selection environment variable.
App redraws wait for GPU setup to complete.

Each mapped toplevel gets an independent quad and swapchain. Windows occupy
stable, nonoverlapping positions, starting in the center and alternating right
and left. Commits and resizing do not move other windows; closed-window positions
can be reused. Unmapping or closing removes the quad, and remapping restores it.
The targeting cross and pointer select the nearest intersected window, and
clicking transfers keyboard focus. Closing or unmapping the active window hands
focus to another mapped window.

Images use the window's logical size and buffer scale, rather than a fixed
512-pixel texture. Large windows are scaled proportionally to fit the runtime's
swapchain limits, the Vulkan device limit, and a default 4096-pixel per-dimension
cap. Set `SPACETOP_MAX_PANEL_SIZE` to a positive integer to change that cap.
Startup logs report the negotiated image size and composition-layer capacity.
Exceeding the runtime's mapped-window layer limit stops the compositor with a
clear error; windows are not silently hidden.

This is not yet a fully pipelined renderer: each app update allocates a fresh
shared image, producer completion is waited on by the CPU, and Vulkan waits for
copies before releasing swapchain images. Pending updates coalesce to the latest
image per window without dropping unmap/close notifications. Buffer pooling and
explicit GPU semaphore handoff remain follow-ups. Popup/menu rendering and
window movement/resizing controls are not implemented yet.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

An opt-in hardware test creates a real Wayland app, composites through GLES,
imports into Vulkan, checks image-copy and cursor pixels, exercises GPU-backed
client buffers, and verifies deferred GPU setup, multiwindow focus and nearest
hits, clicks, redraws, 1600x800 capture, resize, unmap/remap, close, placement reuse,
update coalescing, and layer-limit enforcement.
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
pointer input, keyboard focus, and close. The GPU-backed test also verifies
captured pixels, application-requested resize, unmap/remap, and placement reuse:

```sh
cargo test x11_app -- --ignored --nocapture --test-threads=1
```

These require `Xwayland` and a valid `XDG_RUNTIME_DIR`; the GPU-backed test also
requires a compatible render node and Vulkan DMA-BUF import support.
