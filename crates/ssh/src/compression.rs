//! Compress SSH traffic, including forwarded X11 channels, when the peer supports it.

pub(crate) fn preferred() -> russh::Preferred {
    use russh::compression::{NONE, ZLIB, ZLIB_LEGACY};
    russh::Preferred {
        // russh calls the delayed OpenSSH variant ZLIB_LEGACY; prefer its wire
        // protocol name zlib@openssh.com, then zlib, with uncompressed fallback.
        compression: std::borrow::Cow::Borrowed(&[ZLIB_LEGACY, ZLIB, NONE]),
        ..Default::default()
    }
}
