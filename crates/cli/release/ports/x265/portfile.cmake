vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO Multicorewareinc/x265
    REF "${VERSION}"
    SHA512 270d0db180ecebfc5c2f1fe6451be66042fab69273d073428f4af00420fa0f8b792ecdc71ec1a15ee2860e3b6a325c56295143f117e94fa3616f0ccecb2ded75
    HEAD_REF master
    PATCHES
        disable-install-pdb.patch
        version.patch
        linkage.diff
        pkgconfig.diff
        pthread.diff
        compiler-target.diff
        neon.diff
        advapi32.patch # Required since v4.2 as it is now using RegOpenKeyExA, RegQueryValueExA & RegCloseKey
)

# vcpkg's x265 port with two changes for the lumen binary, which is the only thing that links
# this library: assembly stays on in static Linux builds, and a 10-bit build is linked into the
# 8-bit library, so one libx265 encodes both depths.
set(OPTIONS
    -DENABLE_SHARED=OFF
    -DENABLE_CLI=OFF
    -DENABLE_PIC=ON
    -DENABLE_LIBNUMA=OFF
    "-DVERSION=${VERSION}"
)
if(VCPKG_TARGET_ARCHITECTURE STREQUAL "x64")
    vcpkg_find_acquire_program(NASM)
    list(APPEND OPTIONS "-DNASM_EXECUTABLE=${NASM}")
endif()

vcpkg_cmake_configure(
    SOURCE_PATH "${SOURCE_PATH}/source"
    OPTIONS
        ${OPTIONS}
        -DHIGH_BIT_DEPTH=ON
        -DEXPORT_C_API=OFF
)
vcpkg_cmake_build()
set(MAIN10 "${CURRENT_BUILDTREES_DIR}/libx265_main10.a")
file(COPY_FILE "${CURRENT_BUILDTREES_DIR}/${TARGET_TRIPLET}-rel/libx265.a" "${MAIN10}")

vcpkg_cmake_configure(
    SOURCE_PATH "${SOURCE_PATH}/source"
    OPTIONS
        ${OPTIONS}
        -DLINKED_10BIT=ON
        "-DEXTRA_LIB=${MAIN10}"
)
vcpkg_cmake_install()
file(INSTALL "${MAIN10}" DESTINATION "${CURRENT_PACKAGES_DIR}/lib")
vcpkg_replace_string("${CURRENT_PACKAGES_DIR}/lib/pkgconfig/x265.pc" "-lx265" "-lx265 -lx265_main10")
vcpkg_fixup_pkgconfig()

file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug/include")

vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/COPYING")
