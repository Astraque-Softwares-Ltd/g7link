// SPDX-License-Identifier: GPL-3.0-or-later
#if defined(TEST_BUILTIN) || defined(TEST_PLUGIN)
#ifdef TEST_BUILTIN
#include "../cpp/linux_video_builtin.h"
#endif
#include "../cpp/linux_video_codec.h"

#include <cstdio>
#include <cstdlib>
#include <string>

#ifndef TEST_LABEL
#define TEST_LABEL "test"
#endif
namespace {
int object;

// Whether this adapter's label is in the space-separated list in `variable`: the test
// main sets them to make adapters act as if their FFmpeg were missing, or had no encoder.
bool named_in(const char* variable)
{
    const char* names = std::getenv(variable);
    return names && (" " + std::string(names) + " ").find(" " TEST_LABEL " ") != std::string::npos;
}
}

#ifdef TEST_BUILTIN
#define TEST_RUNTIME_AVAILABLE builtin_video_runtime_available
#else
#define TEST_RUNTIME_AVAILABLE g7link_video_runtime_available
#endif
extern "C" bool TEST_RUNTIME_AVAILABLE() { return !named_in("G7LINK_TEST_MISSING"); }

extern "C" void* g7link_video_decoder_create(bool, char*, size_t)
{
#ifdef TEST_AVAILABLE
    return &object;
#else
    return nullptr;
#endif
}
extern "C" void g7link_video_decoder_destroy(void*) {}
extern "C" bool g7link_video_decoder_next_device(void* p, char*, size_t) { return p == &object; }
extern "C" const char* g7link_video_decoder_backend(const void*) { return TEST_LABEL; }
extern "C" int g7link_video_decoder_send(void* p, const uint8_t*, size_t n, char*, size_t)
{ return p == &object ? static_cast<int>(n) : -1; }
extern "C" int g7link_video_decoder_receive(void*, G7LinkDecodedNv12*, char*, size_t) { return 0; }
extern "C" void g7link_video_decoder_flush(void*) {}
extern "C" void* g7link_video_encoder_create(bool, int, int, int, int, char* error, size_t capacity)
{
    // What an adapter whose FFmpeg is missing says, though the dispatcher shouldn't ask it.
    if (named_in("G7LINK_TEST_MISSING")) {
        std::snprintf(error, capacity, "Matching FFmpeg runtime libraries are unavailable");
        return nullptr;
    }
    if (named_in("G7LINK_TEST_FAILING")) {
        std::snprintf(error, capacity, "%s", TEST_LABEL " has no hardware encoder");
        return nullptr;
    }
#ifdef TEST_AVAILABLE
    return &object;
#else
    return nullptr;
#endif
}
extern "C" void g7link_video_encoder_destroy(void*) {}
extern "C" bool g7link_video_encoder_next_device(void* p, char*, size_t) { return p == &object; }
extern "C" const char* g7link_video_encoder_backend(const void*) { return TEST_LABEL; }
extern "C" int g7link_video_encoder_send(void* p, const uint8_t*, size_t n, int64_t, char*, size_t)
{ return p == &object ? static_cast<int>(n) : -1; }
extern "C" int g7link_video_encoder_receive(void*, const uint8_t**, size_t*, bool*, char*, size_t) { return 0; }
extern "C" void g7link_video_encoder_request_keyframe(void*) {}
extern "C" bool g7link_video_encoder_set_bitrate(void* p, int bitrate) { return p == &object && bitrate == 123; }
extern "C" void* g7link_video_aac_encoder_create(int, int, char*, size_t)
{
#ifdef TEST_AVAILABLE
    return &object;
#else
    return nullptr;
#endif
}
extern "C" void g7link_video_aac_encoder_destroy(void*) {}
extern "C" int g7link_video_aac_encoder_send(void* p, const int16_t*, size_t n, char*, size_t)
{ return p == &object ? static_cast<int>(n) : -1; }
extern "C" int g7link_video_aac_encoder_receive(void*, const uint8_t**, size_t*, char*, size_t) { return 0; }
extern "C" void* g7link_video_aac_decoder_create(char*, size_t)
{
#ifdef TEST_AVAILABLE
    return &object;
#else
    return nullptr;
#endif
}
extern "C" void g7link_video_aac_decoder_destroy(void*) {}
extern "C" int g7link_video_aac_decoder_send(void* p, const uint8_t*, size_t n, char*, size_t)
{ return p == &object ? static_cast<int>(n) : -1; }
extern "C" int g7link_video_aac_decoder_receive(void*, const int16_t**, size_t*, int*, int*, char*, size_t)
{ return 0; }

#else
#include "../cpp/linux_video_codec.h"
#include <cassert>
#include <cstdlib>
#include <cstring>

static bool starts_with(const char* text, const char* prefix)
{
    return std::strncmp(text, prefix, std::strlen(prefix)) == 0;
}

// With adapters for FFmpeg 7 (no encoder), 8 and 9 in place: when 9's FFmpeg isn't installed
// and 8 has no encoder, the reason is 8's, not 9's "Matching FFmpeg runtime libraries are
// unavailable" that used to hide it; the built-in adapter is the last resort.
static void reason_comes_from_an_installed_ffmpeg()
{
    setenv("G7LINK_TEST_MISSING", "ffmpeg9", 1);
    setenv("G7LINK_TEST_FAILING", "ffmpeg8", 1);
    char why[512]{};
    void* encoder = g7link_video_encoder_create(false, 320, 240, 30, 123, why, sizeof(why));
    if (encoder) {
        assert(std::strcmp(g7link_video_encoder_backend(encoder), "builtin") == 0);
        g7link_video_encoder_destroy(encoder);
    } else {
        assert(starts_with(why, "ffmpeg8: ffmpeg8 has no hardware encoder ("));
        assert(!std::strstr(why, "Matching FFmpeg runtime libraries are unavailable"));
        assert(std::strstr(why, "ffmpeg9: FFmpeg isn't installed"));
        assert(std::strstr(why, "ffmpeg7: unavailable"));
    }
    // Nothing installed at all says so.
    setenv("G7LINK_TEST_MISSING", "ffmpeg9 ffmpeg8 test builtin", 1);
    why[0] = '\0';
    assert(!g7link_video_encoder_create(false, 320, 240, 30, 123, why, sizeof(why)));
    assert(starts_with(why, "no installed FFmpeg (ffmpeg9: FFmpeg isn't installed; ffmpeg8: FFmpeg isn't"));
    unsetenv("G7LINK_TEST_MISSING");
    unsetenv("G7LINK_TEST_FAILING");
}

int main(int argc, char** argv)
{
    char error[128]{};
    auto* encoder = g7link_video_encoder_create(false, 320, 240, 30, 123, error, sizeof(error));
    auto* decoder = g7link_video_decoder_create(false, error, sizeof(error));
    auto* aac_encoder = g7link_video_aac_encoder_create(48000, 2, error, sizeof(error));
    auto* aac_decoder = g7link_video_aac_decoder_create(error, sizeof(error));
    if (argc > 1 && std::strcmp(argv[1], "none") == 0) {
        assert(!encoder && !decoder && !aac_encoder && !aac_decoder);
        return 0;
    }
    assert(encoder && decoder && aac_encoder && aac_decoder);
    const char* expected = argc > 1 ? argv[1] : "test";
    assert(std::strcmp(g7link_video_encoder_backend(encoder), expected) == 0);
    assert(std::strcmp(g7link_video_decoder_backend(decoder), expected) == 0);
    const uint8_t data[] = {1, 2, 3};
    assert(g7link_video_encoder_send(encoder, data, sizeof(data), 0, error, sizeof(error)) == 3);
    assert(g7link_video_decoder_send(decoder, data, sizeof(data), error, sizeof(error)) == 3);
    const int16_t pcm[] = {10, 20, 30, 40};
    assert(g7link_video_aac_encoder_send(aac_encoder, pcm, 2, error, sizeof(error)) == 2);
    assert(g7link_video_aac_decoder_send(aac_decoder, data, sizeof(data), error, sizeof(error)) == 3);
    G7LinkDecodedNv12 frame{};
    assert(g7link_video_decoder_receive(decoder, &frame, error, sizeof(error)) == 0);
    const uint8_t* packet = nullptr;
    size_t length = 0;
    bool keyframe = false;
    assert(g7link_video_encoder_receive(encoder, &packet, &length, &keyframe, error, sizeof(error)) == 0);
    assert(g7link_video_aac_encoder_receive(aac_encoder, &packet, &length, error, sizeof(error)) == 0);
    const int16_t* decoded_pcm = nullptr;
    size_t decoded_frames = 0;
    int sr = 0, ch = 0;
    assert(g7link_video_aac_decoder_receive(aac_decoder, &decoded_pcm, &decoded_frames, &sr, &ch, error, sizeof(error)) == 0);
    assert(g7link_video_encoder_set_bitrate(encoder, 123));
    assert(g7link_video_encoder_next_device(encoder, error, sizeof(error)));
    assert(g7link_video_decoder_next_device(decoder, error, sizeof(error)));
    g7link_video_encoder_request_keyframe(encoder);
    g7link_video_decoder_flush(decoder);
    g7link_video_encoder_destroy(encoder);
    g7link_video_decoder_destroy(decoder);
    g7link_video_aac_encoder_destroy(aac_encoder);
    g7link_video_aac_decoder_destroy(aac_decoder);
    if (std::strcmp(expected, "ffmpeg9") == 0) reason_comes_from_an_installed_ffmpeg();
}
#endif
