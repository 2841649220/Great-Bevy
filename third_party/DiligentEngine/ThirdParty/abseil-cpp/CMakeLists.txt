# No-op stub for abseil-cpp.
#
# The real abseil-cpp is fetched via FetchContent from
# chromium.googlesource.com, which is unreachable in this offline-ish
# environment. Nothing in a D3D12-only DiligentCore build links abseil, so
# diligent-sys points FETCHCONTENT_SOURCE_DIR_ABSEIL-CPP at this directory to
# make FetchContent_MakeAvailable() a no-op instead of a network fetch.
cmake_minimum_required (VERSION 3.11)

project(abseil-cpp-stub NONE)
