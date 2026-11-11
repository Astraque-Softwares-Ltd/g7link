// SPDX-License-Identifier: GPL-3.0-or-later
// Stands in for cxx's header in the C++ tests: the two types video_view.h
// takes from Rust, with the members it uses.
#pragma once

#include <cstddef>

namespace rust {

class Str
{
public:
    Str(const char *data, std::size_t size) : m_data(data), m_size(size) {}
    const char *data() const { return m_data; }
    std::size_t size() const { return m_size; }

private:
    const char *m_data;
    std::size_t m_size;
};

template <typename T>
class Slice
{
public:
    Slice(T *data, std::size_t size) : m_data(data), m_size(size) {}
    T *data() const { return m_data; }
    std::size_t size() const { return m_size; }

private:
    T *m_data;
    std::size_t m_size;
};

} // namespace rust
