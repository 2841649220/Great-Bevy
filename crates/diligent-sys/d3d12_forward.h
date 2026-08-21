// Forward declarations of the Direct3D12 types `RenderDeviceD3D12.h`
// references in its method signatures (ID3D12Device*, ID3D12Resource*, ...).
//
// Diligent's C++ sources include <d3d12.h> before the interface header, but
// bindgen cannot do that: <d3d12.h> pulls in the COM headers (objidl.h, ...)
// whose `typedef enum tagBIND_FLAGS BIND_FLAGS` collides with Diligent's
// C-mode `typedef Uint32 BIND_FLAGS` (GraphicsTypes.h:128) - the same clash
// that excludes LoadEngineDll.h from the bindgen set.
//
// Forward-declaring the structs (in the Windows-SDK form `typedef struct
// ID3D12Device ID3D12Device;`) keeps the signatures concrete for bindgen
// while sidestepping the full Windows SDK include chain. The generated
// bindings then carry `*mut ID3D12Device` / `*mut ID3D12Resource` pointers
// that the native device handle escape hatch (M5a, task 16.1) hands to
// vendor SDKs as raw handles.
//
// This header is NOT part of the public interface; it is only consumed by
// diligent-sys/build.rs via clang `-include`.
typedef struct ID3D12Device ID3D12Device;
typedef struct ID3D12Resource ID3D12Resource;
typedef struct ID3D12CommandQueue ID3D12CommandQueue;
typedef struct ID3D12CommandList ID3D12CommandList;
typedef struct ID3D12GraphicsCommandList ID3D12GraphicsCommandList;
typedef struct ID3D12CommandAllocator ID3D12CommandAllocator;
typedef struct ID3D12DescriptorHeap ID3D12DescriptorHeap;
typedef struct ID3D12PipelineState ID3D12PipelineState;
typedef struct ID3D12RootSignature ID3D12RootSignature;
typedef struct ID3D12Fence ID3D12Fence;
typedef struct ID3D12QueryHeap ID3D12QueryHeap;
