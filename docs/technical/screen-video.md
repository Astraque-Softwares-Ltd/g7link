# Screen video implementation

Linux tries FFmpeg's NVENC, VAAPI and QSV hardware encoders, with OpenH264 as
the final H.264 fallback. It tries available CUDA devices and DRM render nodes
for hardware decoding before falling back to software. Decoded pictures are
downloaded as NV12 for the Qt renderer. Linux advertises `mirror.decode.hevc`
when its hardware HEVC decoder initializes.
