//! Bounded signature classification, not decoding. Never trusts filename/MIME.
pub(super) fn content_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        "audio/wav"
    } else if bytes.starts_with(b"fLaC") {
        "audio/flac"
    } else if bytes.starts_with(b"ID3") && bytes.get(3).is_some_and(|v| (2..=4).contains(v))
        || bytes.len() >= 4
            && bytes[0] == 0xff
            && bytes[1] & 0xe0 == 0xe0
            && bytes[1] & 0x18 != 0x08
            && bytes[1] & 0x06 != 0
            && bytes[2] & 0xf0 != 0xf0
            && bytes[2] & 0x0c != 0x0c
    {
        "audio/mpeg"
    } else if bytes.len() >= 7 && bytes[0] == 0xff && bytes[1] & 0xf6 == 0xf0 {
        "audio/aac"
    } else if bytes.starts_with(b"OggS\0") {
        if contains(bytes, b"OpusHead") || contains(bytes, b"\x01vorbis") {
            "audio/ogg"
        } else if contains(bytes, b"\x80theora") {
            "video/ogg"
        } else {
            "application/octet-stream"
        }
    } else if bytes.starts_with(b"\x1a\x45\xdf\xa3") && contains(bytes, b"\x42\x82\x84webm") {
        if contains(bytes, b"\x83\x81\x02") && !contains(bytes, b"\x83\x81\x01") {
            "audio/webm"
        } else {
            "video/webm"
        }
    } else if bytes.get(4..8) == Some(b"ftyp") {
        bmff(bytes)
    } else {
        let source = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
        let source = source.trim_ascii_start();
        let source = if source.starts_with(b"<?xml") {
            source
                .windows(2)
                .position(|w| w == b"?>")
                .map_or(source, |end| source[end + 2..].trim_ascii_start())
        } else {
            source
        };
        let mut source = source;
        while source.starts_with(b"<!--") {
            let Some(end) = source.windows(3).position(|w| w == b"-->") else {
                break;
            };
            source = source[end + 3..].trim_ascii_start();
        }
        if tag(source, b"<svg") {
            "image/svg+xml"
        } else if [
            b"<html".as_slice(),
            b"<head",
            b"<body",
            b"<div",
            b"<p",
            b"<h1",
            b"<h2",
            b"<h3",
            b"<table",
            b"<script",
            b"<style",
            b"<canvas",
            b"<section",
        ]
        .iter()
        .any(|start| tag(source, start))
            || source
                .get(..14)
                .is_some_and(|s| s.eq_ignore_ascii_case(b"<!doctype html"))
        {
            "text/html"
        } else {
            "application/octet-stream"
        }
    }
}
fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle)
}
fn tag(source: &[u8], start: &[u8]) -> bool {
    source
        .get(..start.len())
        .is_some_and(|s| s.eq_ignore_ascii_case(start))
        && source
            .get(start.len())
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b'>')
}
fn bmff(bytes: &[u8]) -> &'static str {
    let Some(length) = bytes
        .get(..4)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_be_bytes)
        .and_then(|n| usize::try_from(n).ok())
    else {
        return "application/octet-stream";
    };
    if length < 16 || length > bytes.len() || length % 4 != 0 {
        return "application/octet-stream";
    }
    let has_brand = |wanted: &[&[u8]]| {
        bytes[8..length]
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .any(|(i, brand)| i != 1 && wanted.contains(&brand.as_slice()))
    };
    if has_brand(&[b"avif", b"avis"]) {
        "image/avif"
    } else if matches!(bytes.get(8..12), Some(b"M4A " | b"M4B ")) {
        "audio/mp4"
    }
    // HEIC/HEIF are not video just because they share the ISO container.
    else if has_brand(&[b"heic", b"heix", b"mif1", b"msf1"]) {
        "application/octet-stream"
    } else if has_brand(&[
        b"isom", b"iso2", b"mp41", b"mp42", b"avc1", b"M4V ", b"dash",
    ]) {
        "video/mp4"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_media_signatures_and_active_documents() {
        for (bytes, expected) in [
            (b"\x89PNG\r\n\x1a\n".as_slice(), "image/png"),
            (b"RIFF1234WAVEfmt ", "audio/wav"),
            (b"fLaCdata", "audio/flac"),
            (b"ID3\x04\0\0", "audio/mpeg"),
            (b"\xff\xfb\x90\x64", "audio/mpeg"),
            (b"\xff\xf1\x50\x80\x01\x7f\xfc", "audio/aac"),
            (b"OggS\0headerOpusHead", "audio/ogg"),
            (b"\x1a\x45\xdf\xa3\x42\x82\x84webm", "video/webm"),
            (b"\0\0\0\x18ftypavif\0\0\0\0avifmif1", "image/avif"),
            (b"\0\0\0\x14ftypM4A \0\0\0\0isom", "audio/mp4"),
            (b"\0\0\0\x14ftypisom\0\0\0\0mp42", "video/mp4"),
            (
                b"\0\0\0\x14ftypheic\0\0\0\0mif1",
                "application/octet-stream",
            ),
            (b"\xef\xbb\xbf  <!DOCTYPE html><html>", "text/html"),
            (b"<?xml version='1.0'?>\n<svg xmlns='x'>", "image/svg+xml"),
            (b"<svg-malicious>", "application/octet-stream"),
            (b"ID3\xff", "application/octet-stream"),
            (b"\xff\xff\xff\xff", "application/octet-stream"),
            (b"arbitrary text or executable", "application/octet-stream"),
        ] {
            assert_eq!(content_type(bytes), expected);
        }
        for length in 0..64 {
            let bytes = vec![0xff; length];
            let _ = content_type(&bytes);
        }
    }
}
