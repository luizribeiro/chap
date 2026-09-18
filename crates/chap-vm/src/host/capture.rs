use std::{collections::VecDeque, format, vec::Vec};

pub(super) struct StreamCapture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    head_limit: usize,
    tail_limit: usize,
    omitted: u64,
}

impl StreamCapture {
    pub(super) fn new(limit: u64) -> Self {
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let head_limit = limit / 2;
        Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            head_limit,
            tail_limit: limit - head_limit,
            omitted: 0,
        }
    }

    pub(super) fn append(&mut self, bytes: &[u8]) {
        let head_bytes = (self.head_limit - self.head.len()).min(bytes.len());
        self.head.extend_from_slice(&bytes[..head_bytes]);
        let bytes = &bytes[head_bytes..];
        if bytes.is_empty() {
            return;
        }

        if bytes.len() >= self.tail_limit {
            self.omitted = self.omitted.saturating_add(
                u64::try_from(self.tail.len() + bytes.len() - self.tail_limit).unwrap_or(u64::MAX),
            );
            self.tail.clear();
            self.tail.extend(&bytes[bytes.len() - self.tail_limit..]);
            return;
        }

        let evicted = (self.tail.len() + bytes.len()).saturating_sub(self.tail_limit);
        self.omitted = self
            .omitted
            .saturating_add(u64::try_from(evicted).unwrap_or(u64::MAX));
        self.tail.drain(..evicted);
        self.tail.extend(bytes);
    }

    pub(super) fn truncated(&self) -> bool {
        self.omitted > 0
    }

    pub(super) fn into_bytes(mut self) -> Vec<u8> {
        if self.truncated() {
            self.head.extend_from_slice(
                format!("\n[... {} bytes omitted ...]\n", self.omitted).as_bytes(),
            );
        }
        self.head.extend(self.tail);
        self.head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(limit: u64, chunks: &[&[u8]]) -> StreamCapture {
        let mut capture = StreamCapture::new(limit);
        for chunk in chunks {
            capture.append(chunk);
        }
        capture
    }

    #[test]
    fn output_shorter_than_the_limit_passes_through() {
        let capture = capture(16, &[b"01\n02\n03\n"]);

        assert!(!capture.truncated());
        assert_eq!(capture.into_bytes(), b"01\n02\n03\n");
    }

    #[test]
    fn output_exactly_at_the_limit_passes_through() {
        let capture = capture(12, &[b"01\n02\n03\n04\n"]);

        assert!(!capture.truncated());
        assert_eq!(capture.into_bytes(), b"01\n02\n03\n04\n");
    }

    #[test]
    fn output_over_the_limit_keeps_the_head_marker_and_tail() {
        let capture = capture(12, &[b"01\n02\n03\n04\n05\n06\n"]);

        assert!(capture.truncated());
        assert_eq!(
            capture.into_bytes(),
            b"01\n02\n\n[... 6 bytes omitted ...]\n05\n06\n"
        );
    }

    #[test]
    fn one_chunk_can_straddle_the_head_and_tail() {
        let capture = capture(10, &[b"01\n", b"02\n03\n04\n"]);

        assert!(capture.truncated());
        assert_eq!(
            capture.into_bytes(),
            b"01\n02\n[... 2 bytes omitted ...]\n3\n04\n"
        );
    }

    #[test]
    fn a_chunk_larger_than_the_tail_replaces_older_tail_bytes() {
        let capture = capture(12, &[b"01\n02\n03\n", b"04\n05\n06\n"]);

        assert!(capture.truncated());
        assert_eq!(
            capture.into_bytes(),
            b"01\n02\n\n[... 6 bytes omitted ...]\n05\n06\n"
        );
    }

    #[test]
    fn an_odd_limit_gives_the_extra_byte_to_the_tail() {
        let capture = capture(11, &[b"01\n02\n03\n04\n05\n"]);

        assert!(capture.truncated());
        assert_eq!(
            capture.into_bytes(),
            b"01\n02\n[... 4 bytes omitted ...]\n04\n05\n"
        );
    }
}
