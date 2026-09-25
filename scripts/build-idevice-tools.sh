#!/usr/bin/env bash
# Builds the pinned libimobiledevice tools (idevice_id, ideviceinfo, idevicepair, idevicebackup2)
# for one platform and packs them into idevice-tools-<version>-<platform>.zip (ROADMAP X1,
# docs/IDEVICE-CLI.md §1).
#
# Usage: scripts/build-idevice-tools.sh <platform-key>
#   macos-aarch64, macos-x86_64   on a Mac (x86_64 is cross-built with -arch x86_64)
#   windows-x86_64                in an MSYS2 UCRT64 shell (a per-user install needs no admin)
#
# The build is scripted, not bit-reproducible. BUILDINFO.json inside the zip records what it used,
# and the script prints the platform entry for idevice-tools.json at the end.
#
# - Every source tarball comes from idevice-tools.json and is verified against its SHA-256 before
#   use. The configure scripts shipped in the tarballs are used as they are (no autoreconf).
# - Every library is built with --enable-static --disable-shared, and pkg-config runs with --static,
#   so the tools link only system libraries. The script checks that (otool -L / objdump -p).
# - Build tools: autoconf, automake, libtool and pkg-config. If autoconf, automake or pkg-config is
#   missing (Homebrew may not be writable), pinned releases are built from source into
#   ~/.local/idevice-buildtools after their hashes are verified, plus GNU M4 when autoconf needs
#   it (the macOS m4 1.4.6 is too old for autoconf 2.73).
#
# Environment:
#   IDEVICE_BUILD_DIR   work directory; the zip is written there too
#                       (default: <repo>/target/idevice-tools-build)
#   IDEVICE_BUILDTOOLS  prefix for bootstrapped build tools (default: ~/.local/idevice-buildtools)
#
# Written for bash 3.2 (the macOS /bin/bash): no associative arrays, no mapfile.
set -euo pipefail

TOOLS="idevice_id ideviceinfo idevicepair idevicebackup2"
MACOS_DEPLOYMENT_TARGET=11.0

# Build tools bootstrapped when missing: name version url sha256 (hashes cross-checked against the
# Homebrew formulae).
BUILDTOOL_PINS="
m4 1.4.21 https://ftp.gnu.org/gnu/m4/m4-1.4.21.tar.xz f25c6ab51548a73a75558742fb031e0625d6485fe5f9155949d6486a2408ab66
autoconf 2.73 https://ftp.gnu.org/gnu/autoconf/autoconf-2.73.tar.gz 259ddfa3bddc799cfb81489cc0f17dfdf1bd6d1505dda53c0f45ff60d6a4f9a7
automake 1.19 https://ftp.gnu.org/gnu/automake/automake-1.19.tar.xz e3e2c2e3abf37898138db5b6c1d1dc35c9160c5978be7947d2c741705251d445
pkgconf 3.0.7 https://distfiles.ariadne.space/pkgconf/pkgconf-3.0.7.tar.xz c926ff491cbd9a331a589160811bd97ab1749b4d5198a519338f2cdfabe6940a
"

# The 3rd_party/ license files are not in the libimobiledevice release tarball; they are taken from
# the 1.4.0 tag (hashes checked against the git blobs of that tag): bundle path, url, sha256.
NOTICE_PINS="
3rd_party/ed25519/LICENSE https://raw.githubusercontent.com/libimobiledevice/libimobiledevice/1.4.0/3rd_party/ed25519/LICENSE f68d76b9c1c2271422e55134ea94cf71f2ac3fc3ed6a7ec6b83a1a0538753214
3rd_party/libsrp6a-sha512/LICENSE https://raw.githubusercontent.com/libimobiledevice/libimobiledevice/1.4.0/3rd_party/libsrp6a-sha512/LICENSE 6b420542f5295bd85e285a2142eca27be4f700e4bd1648450c07be4f82548a85
"

die() {
  echo "build-idevice-tools: $*" >&2
  exit 1
}
log() { echo "==> $*" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# fetch <url> <sha256> <dest>: download over HTTPS only, verify, keep the verified file as a cache.
fetch() {
  local url=$1 want=$2 dest=$3 got
  if [[ -f $dest && "$(sha256_of "$dest")" == "$want" ]]; then
    return
  fi
  log "downloading $url"
  rm -f "$dest.part"
  curl -fsSL --proto '=https' --proto-redir '=https' --retry 3 -o "$dest.part" "$url" </dev/null
  got=$(sha256_of "$dest.part")
  if [[ $got != "$want" ]]; then
    rm -f "$dest.part"
    die "SHA-256 mismatch for $url: got $got, want $want"
  fi
  mv "$dest.part" "$dest"
}

# A JSON string literal (the values used here are single-line).
json_str() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\t'/ }
  printf '"%s"' "$s"
}

# The first line of a command's output, or "unknown".
first_line() {
  local out
  out=$("$@" 2>&1 | head -n 1 | tr -d '\r') || true
  echo "${out:-unknown}"
}

usage() {
  sed -n '6,8p' "$0" >&2
  exit 2
}

[[ $# -eq 1 ]] || usage
platform=$1
case $platform in
  macos-aarch64)
    [[ $(uname -s) == Darwin ]] || die "$platform must be built on macOS"
    arch=arm64
    host=aarch64-apple-darwin
    exe=""
    ;;
  macos-x86_64)
    [[ $(uname -s) == Darwin ]] || die "$platform must be built on macOS"
    arch=x86_64
    host=x86_64-apple-darwin
    exe=""
    ;;
  windows-x86_64)
    [[ ${MSYSTEM:-} == UCRT64 ]] || die "$platform must be built in an MSYS2 UCRT64 shell"
    arch=x86_64
    host=x86_64-w64-mingw32
    exe=".exe"
    ;;
  *) usage ;;
esac

repo_root=$(cd "$(dirname "$0")/.." && pwd)
manifest="$repo_root/idevice-tools.json"
[[ -f $manifest ]] || die "missing $manifest"
build_root="${IDEVICE_BUILD_DIR:-$repo_root/target/idevice-tools-build}"
work="$build_root/$platform"
downloads="$build_root/downloads"
out_dir="$build_root"
buildtools="${IDEVICE_BUILDTOOLS:-$HOME/.local/idevice-buildtools}"
src="$work/src"
prefix="$work/prefix"
stage="$work/stage"
if have nproc; then jobs=$(nproc); else jobs=$(sysctl -n hw.ncpu); fi

# ---- the manifest: version and sources ----

manifest_flat=$(tr -d '\r\n' <"$manifest")
head_part=${manifest_flat%%\"sources\"*}
version=$(grep -oE '"version"[[:space:]]*:[[:space:]]*"[^"]*"' <<<"$head_part" | head -n 1 | sed -E 's/.*"([^"]*)"$/\1/')
[[ -n $version ]] || die "no version in $manifest"
sources_part=${manifest_flat#*\"sources\"}
sources_part=${sources_part%%\"platforms\"*}

SRC_NAMES=()
SRC_VERSIONS=()
SRC_URLS=()
SRC_SHAS=()
while read -r name ver url sha; do
  SRC_NAMES+=("$name")
  SRC_VERSIONS+=("$ver")
  SRC_URLS+=("$url")
  SRC_SHAS+=("$sha")
done < <(
  grep -oE '"(name|version|url|sha256)"[[:space:]]*:[[:space:]]*"[^"]*"' <<<"$sources_part" |
    sed -E 's/^"([a-z0-9]+)"[[:space:]]*:[[:space:]]*"([^"]*)"$/\1 \2/' |
    awk '{ v[$1] = $2 }
         ("name" in v) && ("version" in v) && ("url" in v) && ("sha256" in v) {
           print v["name"], v["version"], v["url"], v["sha256"]; split("", v) }'
)
[[ ${#SRC_NAMES[@]} -gt 0 ]] || die "no sources in $manifest"

# src_dir <name>: the extracted source directory of a manifest source.
src_dir() {
  local i
  for ((i = 0; i < ${#SRC_NAMES[@]}; i++)); do
    if [[ ${SRC_NAMES[$i]} == "$1" ]]; then
      echo "$src/$1-${SRC_VERSIONS[$i]}"
      return
    fi
  done
  die "source $1 is not in $manifest"
}

# ---- build tools ----

bootstrap_buildtool() {
  local want=$1 name ver url sha dir
  while read -r name ver url sha; do
    [[ $name == "$want" ]] || continue
    log "bootstrapping $name $ver into $buildtools"
    mkdir -p "$downloads" "$buildtools/src"
    fetch "$url" "$sha" "$downloads/${url##*/}"
    rm -rf "$buildtools/src/$name-$ver"
    tar -xf "$downloads/${url##*/}" -C "$buildtools/src"
    dir="$buildtools/src/$name-$ver"
    (cd "$dir" && ./configure --prefix="$buildtools" && make -j"$jobs" && make install) \
      </dev/null >"$buildtools/src/$name.log" 2>&1 || die "building $name failed; see $buildtools/src/$name.log"
    if [[ $name == pkgconf ]]; then
      ln -sf pkgconf "$buildtools/bin/pkg-config"
    fi
    return
  done <<<"$BUILDTOOL_PINS"
  die "no pin for build tool $want"
}

# autoconf needs GNU M4 1.4.8 or later (1.4.16 or later recommended).
m4_is_recent() {
  have m4 || return 1
  m4 --version 2>/dev/null | head -n 1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n 1 |
    awk -F. '{ ok = $1 > 1 || ($1 == 1 && ($2 > 4 || ($2 == 4 && $3 >= 16))) } END { exit !ok }'
}

export PATH="$buildtools/bin:$PATH"
if ! have autoconf; then
  m4_is_recent || bootstrap_buildtool m4
  bootstrap_buildtool autoconf
fi
have automake || bootstrap_buildtool automake
have pkg-config || bootstrap_buildtool pkgconf
if have glibtool; then
  libtool_cmd=glibtool
elif libtool --version 2>/dev/null | grep -q GNU; then
  libtool_cmd=libtool
else
  die "GNU libtool is required (glibtool or libtool on PATH)"
fi
for tool in make curl tar zip; do
  have "$tool" || die "$tool is required"
done

# ---- toolchain environment ----

# Keep every pkg-config lookup inside our prefix (no Homebrew curl or other host libraries).
export PKG_CONFIG="pkg-config --static"
export PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig"
export PKG_CONFIG_PATH=""
case $platform in
  macos-*)
    export MACOSX_DEPLOYMENT_TARGET=$MACOS_DEPLOYMENT_TARGET
    export CC="clang -arch $arch"
    export CXX="clang++ -arch $arch"
    export CFLAGS="-O2 -mmacosx-version-min=$MACOS_DEPLOYMENT_TARGET"
    export CXXFLAGS="$CFLAGS"
    export CPPFLAGS=""
    export LDFLAGS="-mmacosx-version-min=$MACOS_DEPLOYMENT_TARGET"
    sdk=$(xcrun --show-sdk-path)
    # libtatsu links the system libcurl; macOS ships no libcurl.pc.
    libcurl_cflags="-I$sdk/usr/include"
    libcurl_libs="-lcurl"
    tools_ldflags="$LDFLAGS"
    mbedtls_libs="-lmbedtls -lmbedx509 -lmbedcrypto"
    ;;
  windows-*)
    export CC=gcc
    export CXX=g++
    export CFLAGS="-O2"
    export CXXFLAGS="$CFLAGS"
    # Consumers of the static libraries must not declare their symbols dllimport.
    export CPPFLAGS="-DLIBPLIST_STATIC -DLIMD_GLUE_STATIC -DLIBUSBMUXD_STATIC -DLIBTATSU_STATIC -DLIBIMOBILEDEVICE_STATIC"
    export LDFLAGS="-static-libgcc"
    # libtatsu needs the libcurl headers only: none of the four tools links libtatsu.
    libcurl_cflags=$(PKG_CONFIG_LIBDIR=/ucrt64/lib/pkgconfig pkg-config --cflags libcurl)
    libcurl_cflags=${libcurl_cflags:- }
    libcurl_libs=$(PKG_CONFIG_LIBDIR=/ucrt64/lib/pkgconfig pkg-config --libs libcurl)
    # libtool passes -static to the compiler for programs linked with -all-static.
    tools_ldflags="$LDFLAGS -all-static"
    # mbedtls gathers entropy with BCryptGenRandom.
    mbedtls_libs="-lmbedtls -lmbedx509 -lmbedcrypto -lbcrypt"
    ;;
esac

# ---- sources ----

rm -rf "$src" "$prefix" "$stage"
mkdir -p "$src" "$prefix" "$stage" "$downloads" "$out_dir"
for ((i = 0; i < ${#SRC_NAMES[@]}; i++)); do
  url=${SRC_URLS[$i]}
  fetch "$url" "${SRC_SHAS[$i]}" "$downloads/${url##*/}"
  tar -xf "$downloads/${url##*/}" -C "$src"
  [[ -d $(src_dir "${SRC_NAMES[$i]}") ]] || die "unexpected layout in ${url##*/}"
done
notice_paths=()
while read -r path url sha; do
  [[ -n $path ]] || continue
  fetch "$url" "$sha" "$downloads/${path//\//_}"
  notice_paths+=("$path")
done <<<"$NOTICE_PINS"

# ---- build ----

common_flags="--prefix=$prefix --host=$host --enable-static --disable-shared --disable-dependency-tracking"
flags_libplist="$common_flags --without-cython --without-tests --without-tools"
flags_glue="$common_flags"
flags_usbmuxd="$common_flags"
flags_tatsu="$common_flags"
flags_limd="$common_flags --with-mbedtls --without-cython --without-readline"

# configure_in <dir> <flags> [VAR=value...]
configure_in() {
  local dir=$1 flags=$2
  shift 2
  log "configuring ${dir##*/}"
  # shellcheck disable=SC2086 # the flags are word lists without spaces inside words
  (cd "$dir" && ./configure $flags "$@" >"$work/${dir##*/}.configure.log" 2>&1) ||
    die "configure failed in ${dir##*/}; see $work/${dir##*/}.configure.log"
}

# make_in <dir> [make args...]
make_in() {
  local dir=$1
  shift
  (cd "$dir" && make -j"$jobs" "$@" >>"$work/${dir##*/}.make.log" 2>&1) ||
    die "make $* failed in ${dir##*/}; see $work/${dir##*/}.make.log"
}

d=$(src_dir libplist)
configure_in "$d" "$flags_libplist"
make_in "$d"
make_in "$d" install

d=$(src_dir libimobiledevice-glue)
configure_in "$d" "$flags_glue"
make_in "$d"
make_in "$d" install

d=$(src_dir libusbmuxd)
configure_in "$d" "$flags_usbmuxd"
make_in "$d"
make_in "$d" install

d=$(src_dir libtatsu)
configure_in "$d" "$flags_tatsu" "libcurl_CFLAGS=$libcurl_cflags" "libcurl_LIBS=$libcurl_libs"
make_in "$d"
make_in "$d" install

# mbedtls: the release tarball ships its generated sources; the Makefile build needs no Python.
d=$(src_dir mbedtls)
log "building mbedtls"
make_in "$d/library" static CC="$CC" CFLAGS="$CFLAGS"
mkdir -p "$prefix/include" "$prefix/lib"
cp -R "$d/include/mbedtls" "$d/include/psa" "$prefix/include/"
cp "$d/library/libmbedtls.a" "$d/library/libmbedx509.a" "$d/library/libmbedcrypto.a" "$prefix/lib/"
mbedtls_flags="make -C library static CC=\"$CC\" CFLAGS=\"$CFLAGS\""

# libimobiledevice: only the libraries and the four tools (ideviceimagemounter, the only tool that
# links libtatsu, is not built).
d=$(src_dir libimobiledevice)
limd=$d
configure_in "$d" "$flags_limd" "mbedtls_INCLUDES=$prefix/include" "mbedtls_LIBDIR=$prefix/lib" "mbedtls_LIBS=$mbedtls_libs"
# 3rd_party/libsrp6a-sha512 reads $(mbedtls_CFLAGS), which configure never sets (it sets
# ssl_lib_CFLAGS), so pass it to make.
make_in "$d/3rd_party" "mbedtls_CFLAGS=-I$prefix/include"
make_in "$d/common"
make_in "$d/src"
tool_targets=""
for t in $TOOLS; do tool_targets="$tool_targets $t$exe"; done
# shellcheck disable=SC2086
make_in "$d/tools" LDFLAGS="$tools_ldflags" $tool_targets

# ---- check the tools ----

for t in $TOOLS; do
  f="$limd/tools/$t$exe"
  [[ -f $f ]] || die "missing $f"
  cp "$f" "$stage/$t$exe"
done

linked_json=""
for t in $TOOLS; do
  f="$stage/$t$exe"
  case $platform in
    macos-*)
      [[ $(lipo -archs "$f") == "$arch" ]] || die "$t is not a $arch binary: $(lipo -archs "$f")"
      minos=$(otool -l "$f" | awk '/LC_BUILD_VERSION/ { p = 1 } p && $1 == "minos" { print $2; exit }')
      [[ $minos == "$MACOS_DEPLOYMENT_TARGET" ]] || die "$t has minos $minos"
      libs=$(otool -L "$f" | tail -n +2 | awk '{ print $1 }')
      bad=$(grep -vE '^(/usr/lib/|/System/)' <<<"$libs" || true)
      [[ -z $bad ]] || die "$t links non-system libraries: $bad"
      ;;
    windows-*)
      libs=$(objdump -p "$f" | awk '/DLL Name:/ { print $3 }')
      bad=""
      for dll in $libs; do
        if [[ -n $(find /ucrt64/bin /usr/bin -maxdepth 1 -iname "$dll" 2>/dev/null) ]]; then
          bad="$bad $dll"
        fi
      done
      [[ -z $bad ]] || die "$t imports MSYS2 DLLs:$bad"
      ;;
  esac
  list=""
  for l in $libs; do list="$list${list:+, }$(json_str "$l")"; done
  linked_json="$linked_json${linked_json:+,
    }$(json_str "$t$exe"): [$list]"
done

# --version must print the pinned version where the binary can run here (x86_64 needs Rosetta).
if "$stage/idevicebackup2$exe" --version >/dev/null 2>&1; then
  for t in $TOOLS; do
    out=$("$stage/$t$exe" --version 2>&1 | tr -d '\r')
    [[ $out == *" $version" ]] || die "$t --version printed '$out', want $version"
    log "$out"
  done
  version_checked=true
else
  log "cannot run $platform binaries on this machine; --version not checked"
  version_checked=false
fi

# ---- notices ----

cp "$limd/COPYING" "$limd/COPYING.LESSER" "$stage/"
mkdir -p "$stage/3rd_party/ed25519" "$stage/3rd_party/libsrp6a-sha512" "$stage/mbedtls"
cp "$limd/3rd_party/README.md" "$stage/3rd_party/README.md"
cp "$limd/3rd_party/ed25519/README.md" "$stage/3rd_party/ed25519/README.md"
cp "$limd/3rd_party/libsrp6a-sha512/README.md" "$stage/3rd_party/libsrp6a-sha512/README.md"
for path in "${notice_paths[@]}"; do
  cp "$downloads/${path//\//_}" "$stage/$path"
done
cp "$(src_dir mbedtls)/LICENSE" "$stage/mbedtls/LICENSE"
# MIT-licensed code compiled into libplist; its notice must accompany copies.
{
  echo "Notices for code embedded in libplist $(basename "$(src_dir libplist)" | sed 's/^libplist-//')"
  for f in src/jsmn.c src/time64.c; do
    echo
    echo "==== libplist/$f ===="
    awk '{ print } /\*\// { exit }' "$(src_dir libplist)/$f"
  done
} >"$stage/libplist-embedded-notices.txt"

# ---- BUILDINFO.json ----

case $platform in
  macos-*)
    build_host="macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion)), $(uname -m)"
    toolchain_extra="\"sdk\": $(json_str "$sdk ($(xcrun --show-sdk-version))"),
    \"macosx_deployment_target\": $(json_str "$MACOS_DEPLOYMENT_TARGET"),"
    ;;
  windows-*)
    build_host="$(uname -s) $(uname -r), MSYS2 $MSYSTEM"
    toolchain_extra="\"msys2_packages\": $(json_str "$(pacman -Q mingw-w64-ucrt-x86_64-gcc mingw-w64-ucrt-x86_64-crt mingw-w64-ucrt-x86_64-binutils 2>/dev/null | tr '\n' ' ' | sed 's/ $//')"),"
    ;;
esac
sources_json=""
for ((i = 0; i < ${#SRC_NAMES[@]}; i++)); do
  sources_json="$sources_json${sources_json:+,
    }{ \"name\": $(json_str "${SRC_NAMES[$i]}"), \"version\": $(json_str "${SRC_VERSIONS[$i]}"), \"url\": $(json_str "${SRC_URLS[$i]}"), \"sha256\": $(json_str "${SRC_SHAS[$i]}") }"
done
notices_json=""
while read -r path url sha; do
  [[ -n $path ]] || continue
  notices_json="$notices_json${notices_json:+,
    }{ \"file\": $(json_str "$path"), \"url\": $(json_str "$url"), \"sha256\": $(json_str "$sha") }"
done <<<"$NOTICE_PINS"
files_json=""
for t in $TOOLS; do
  files_json="$files_json${files_json:+,
    }$(json_str "$t$exe"): $(json_str "$(sha256_of "$stage/$t$exe")")"
done

cat >"$stage/BUILDINFO.json" <<EOF
{
  "name": "idevice-tools",
  "version": $(json_str "$version"),
  "platform": $(json_str "$platform"),
  "host_triplet": $(json_str "$host"),
  "built_at": $(json_str "$(date -u +%Y-%m-%dT%H:%M:%SZ)"),
  "build_host": $(json_str "$build_host"),
  "reproducible": false,
  "build_script_sha256": $(json_str "$(sha256_of "$0")"),
  "sources": [
    $sources_json
  ],
  "notices": [
    $notices_json
  ],
  "toolchain": {
    "cc": $(json_str "$(first_line ${CC%% *} --version)"),
    $toolchain_extra
    "make": $(json_str "$(first_line make --version)"),
    "pkg-config": $(json_str "$(command -v pkg-config) $(first_line pkg-config --version)"),
    "autoconf": $(json_str "$(first_line autoconf --version)"),
    "automake": $(json_str "$(first_line automake --version)"),
    "libtool": $(json_str "$(first_line $libtool_cmd --version)")
  },
  "environment": {
    "CC": $(json_str "$CC"),
    "CFLAGS": $(json_str "$CFLAGS"),
    "CPPFLAGS": $(json_str "$CPPFLAGS"),
    "LDFLAGS": $(json_str "$LDFLAGS"),
    "tools_LDFLAGS": $(json_str "$tools_ldflags"),
    "PKG_CONFIG": $(json_str "$PKG_CONFIG")
  },
  "configure_flags": {
    "libplist": $(json_str "${flags_libplist//$prefix/<prefix>}"),
    "libimobiledevice-glue": $(json_str "${flags_glue//$prefix/<prefix>}"),
    "libusbmuxd": $(json_str "${flags_usbmuxd//$prefix/<prefix>}"),
    "libtatsu": $(json_str "${flags_tatsu//$prefix/<prefix>} libcurl_CFLAGS=$libcurl_cflags libcurl_LIBS=$libcurl_libs"),
    "mbedtls": $(json_str "$mbedtls_flags"),
    "libimobiledevice": $(json_str "${flags_limd//$prefix/<prefix>} mbedtls_INCLUDES=<prefix>/include mbedtls_LIBDIR=<prefix>/lib mbedtls_LIBS=$mbedtls_libs; make -C 3rd_party mbedtls_CFLAGS=-I<prefix>/include; make -C tools LDFLAGS=<tools_LDFLAGS>$tool_targets")
  },
  "version_checked": $version_checked,
  "files": {
    $files_json
  },
  "linked_libraries": {
    $linked_json
  }
}
EOF

# ---- zip ----

bundle="idevice-tools-$version-$platform.zip"
rm -f "$out_dir/$bundle"
(cd "$stage" && zip -q -X -D -r -9 "$out_dir/$bundle" .)
log "wrote $out_dir/$bundle"

# The platform entry for idevice-tools.json.
cat <<EOF
"$platform": {
  "bundle": "$bundle",
  "bundle_sha256": "$(sha256_of "$out_dir/$bundle")",
  "files": {
    $files_json
  }
}
EOF
