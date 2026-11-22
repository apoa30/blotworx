fn main() {
    let mut build = cc::Build::new();
    build.cpp(true);

    if build.get_compiler().is_like_msvc() {
        build.flag("/std:c++20");
        build.flag("/EHsc");
    } else {
        build.flag("-std=c++20");
    }

    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_arch == "aarch64" {
        build.include("../../third_party/sse2neon");
        build.flag_if_supported("-fsigned-char");
        println!("cargo:rerun-if-changed=../../third_party/sse2neon");
    } else if target_arch == "arm" {
        build.flag_if_supported("-fsigned-char");
    }
    build.include("cpp");
    build.include("../../third_party/spike/3rd_party/glm");
    build.include("../../third_party/havoklib/include");
    build.include("../../third_party/havoklib/source");
    build.include("../../third_party/spike/include");
    build.include("../../third_party/spike/3rd_party/pugixml/include");

    build.file("cpp/havok_ffi.cpp");
    build.file("../../third_party/havoklib/source/hk_packfile.cpp");
    build.file("../../third_party/havoklib/source/format_old.cpp");
    build.file("../../third_party/havoklib/source/format_new.cpp");
    build.file("../../third_party/havoklib/source/core_motion.cpp");
    build.file("../../third_party/havoklib/source/core_registry.cpp");
    build.file("../../third_party/havoklib/source/hka_spline_decompressor.cpp");
    build.file("../../third_party/havoklib/source/hka_delta_decompressor.cpp");
    build.file("../../third_party/havoklib/source/packfile/hk_rootlevelcontainer.cpp");
    build.file("../../third_party/havoklib/source/packfile/hka_skeleton.cpp");
    build.file("../../third_party/havoklib/source/packfile/hka_animation_binding.cpp");
    build.file("../../third_party/havoklib/source/packfile/hka_animation_container.cpp");
    build.file("../../third_party/havoklib/source/packfile/hka_animation_interleaved.cpp");
    build.file("../../third_party/havoklib/source/packfile/hka_animation_spline.cpp");
    build.file("../../third_party/havoklib/source/packfile/hka_animation_delta.cpp");
    build.file("../../third_party/havoklib/source/packfile/hka_animation_wavelet.cpp");
    build.file(
        "../../third_party/havoklib/source/packfile/custom/hka_losslesscompressedanimation.cpp",
    );
    build.file(
        "../../third_party/havoklib/source/packfile/hka_animated_reference_frame_default.cpp",
    );
    build.file("../../third_party/havoklib/source/packfile/hkx_environment.cpp");
    build.file("../../third_party/havoklib/source/xml/base.cpp");
    build.file("../../third_party/spike/src/reflector.cpp");
    build.file("../../third_party/spike/src/master_printer.cpp");
    build.file("../../third_party/spike/src/crc32.cpp");
    build.file("../../third_party/spike/src/uni.cpp");
    build.file("../../third_party/spike/3rd_party/pugixml/pugixml.cpp");

    build.compile("havok_decompressor_cpp");

    println!("cargo:rerun-if-changed=cpp");
    println!("cargo:rerun-if-changed=../../third_party/havoklib");
    println!("cargo:rerun-if-changed=../../third_party/spike");
}
