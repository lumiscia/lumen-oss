use std::io::{self, Write};

use super::output::MAX_ARCHIVE_FRAMES;

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_OF_DIRECTORY: u32 = 0x0605_4b50;
/// Bit 11: names are UTF-8.
const UTF8_NAMES: u16 = 1 << 11;
/// 1980-01-01 00:00, the earliest time a ZIP can hold, so archives of the same frames are the same
/// bytes.
const DOS_DATE: u16 = (1 << 5) | 1;
const VERSION_NEEDED: u16 = 10;

struct Entry {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

/// Writes a ZIP whose files are stored, not deflated, one at a time, so the archive never needs more
/// memory than the file being added. Images are already compressed, and a stored entry can be
/// read without a decompressor. It doesn't write ZIP64, so it holds at most [`MAX_ARCHIVE_FRAMES`]
/// files and less than 4 GiB, and says [`ArchiveFull`] when a file would pass either.
pub(super) struct StoredZip<W: Write> {
    out: W,
    written: u64,
    entries: Vec<Entry>,
}

/// The archive can't hold another file.
#[derive(Debug)]
pub(super) enum ArchiveError {
    Full,
    Io(io::Error),
}

impl From<io::Error> for ArchiveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl<W: Write> StoredZip<W> {
    pub(super) fn new(out: W) -> Self {
        Self {
            out,
            written: 0,
            entries: Vec::new(),
        }
    }

    pub(super) fn add(&mut self, name: &str, data: &[u8]) -> Result<(), ArchiveError> {
        // Leaves room for this entry's headers and for the directory, which holds a name twice.
        let headers = 30 + 46 + 2 * name.len() as u64;
        let entry_end = self.written + headers + data.len() as u64;
        if self.entries.len() >= MAX_ARCHIVE_FRAMES as usize
            || entry_end + self.directory_size() + 22 >= u64::from(u32::MAX)
        {
            return Err(ArchiveError::Full);
        }
        let crc = crc32fast::hash(data);
        let (size, offset) = (data.len() as u32, self.written as u32);
        let mut header = Vec::with_capacity(30 + name.len());
        header.extend(LOCAL_HEADER.to_le_bytes());
        header.extend(VERSION_NEEDED.to_le_bytes());
        header.extend(UTF8_NAMES.to_le_bytes());
        header.extend(0_u16.to_le_bytes()); // stored
        header.extend(0_u16.to_le_bytes()); // time
        header.extend(DOS_DATE.to_le_bytes());
        header.extend(crc.to_le_bytes());
        header.extend(size.to_le_bytes()); // compressed
        header.extend(size.to_le_bytes()); // uncompressed
        header.extend((name.len() as u16).to_le_bytes());
        header.extend(0_u16.to_le_bytes()); // extra field
        header.extend(name.as_bytes());
        self.write(&header)?;
        self.write(data)?;
        self.entries.push(Entry {
            name: name.to_string(),
            crc,
            size,
            offset,
        });
        Ok(())
    }

    /// Writes the directory and returns the writer.
    pub(super) fn finish(mut self) -> Result<W, ArchiveError> {
        let directory_offset = self.written as u32;
        let mut directory = Vec::with_capacity(self.directory_size() as usize + 22);
        for entry in &self.entries {
            directory.extend(CENTRAL_HEADER.to_le_bytes());
            directory.extend(VERSION_NEEDED.to_le_bytes()); // made by
            directory.extend(VERSION_NEEDED.to_le_bytes()); // needed
            directory.extend(UTF8_NAMES.to_le_bytes());
            directory.extend(0_u16.to_le_bytes()); // stored
            directory.extend(0_u16.to_le_bytes()); // time
            directory.extend(DOS_DATE.to_le_bytes());
            directory.extend(entry.crc.to_le_bytes());
            directory.extend(entry.size.to_le_bytes());
            directory.extend(entry.size.to_le_bytes());
            directory.extend((entry.name.len() as u16).to_le_bytes());
            directory.extend([0_u8; 2 + 2 + 2 + 2 + 4]); // extra, comment, disk, internal, external
            directory.extend(entry.offset.to_le_bytes());
            directory.extend(entry.name.as_bytes());
        }
        let count = self.entries.len() as u16;
        directory.extend(END_OF_DIRECTORY.to_le_bytes());
        directory.extend([0_u8; 4]); // this disk, directory disk
        directory.extend(count.to_le_bytes());
        directory.extend(count.to_le_bytes());
        directory.extend((self.directory_size() as u32).to_le_bytes());
        directory.extend(directory_offset.to_le_bytes());
        directory.extend(0_u16.to_le_bytes()); // comment
        self.write(&directory)?;
        self.out.flush()?;
        Ok(self.out)
    }

    fn directory_size(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| 46 + entry.name.len() as u64)
            .sum()
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    fn u16_at(bytes: &[u8], at: usize) -> usize {
        u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize
    }

    fn u32_at(bytes: &[u8], at: usize) -> usize {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
    }

    /// The files of a stored ZIP, read through its central directory with their checksums checked.
    pub(in crate::render) fn read_zip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let end = bytes.len() - 22;
        assert_eq!(
            u32_at(bytes, end),
            END_OF_DIRECTORY as usize,
            "end of directory"
        );
        let count = u16_at(bytes, end + 10);
        let mut at = u32_at(bytes, end + 16);
        let mut files = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(bytes, at), CENTRAL_HEADER as usize);
            assert_eq!(u16_at(bytes, at + 10), 0, "stored");
            let (crc, size) = (u32_at(bytes, at + 16), u32_at(bytes, at + 24));
            let (name_len, extra_len, comment_len) = (
                u16_at(bytes, at + 28),
                u16_at(bytes, at + 30),
                u16_at(bytes, at + 32),
            );
            let local = u32_at(bytes, at + 42);
            let name = String::from_utf8(bytes[at + 46..at + 46 + name_len].to_vec()).unwrap();
            assert_eq!(u32_at(bytes, local), LOCAL_HEADER as usize);
            let data_at = local + 30 + u16_at(bytes, local + 26) + u16_at(bytes, local + 28);
            let data = bytes[data_at..data_at + size].to_vec();
            assert_eq!(crc32fast::hash(&data) as usize, crc, "{name}");
            files.push((name, data));
            at += 46 + name_len + extra_len + comment_len;
        }
        files
    }

    #[test]
    fn files_come_back_in_order_with_their_checksums() {
        let mut zip = StoredZip::new(Vec::new());
        zip.add("frame_0000.png", b"first").unwrap();
        zip.add("frame_0001.png", b"").unwrap();
        zip.add("frame_0002.png", &[7; 100_000]).unwrap();
        let bytes = zip.finish().unwrap();

        let files = read_zip(&bytes);
        assert_eq!(
            files
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["frame_0000.png", "frame_0001.png", "frame_0002.png"]
        );
        assert_eq!(files[0].1, b"first");
        assert!(files[1].1.is_empty());
        assert_eq!(files[2].1, vec![7; 100_000]);
    }

    #[test]
    fn an_empty_archive_is_just_a_directory_end() {
        let bytes = StoredZip::new(Vec::new()).finish().unwrap();
        assert_eq!(bytes.len(), 22);
        assert!(read_zip(&bytes).is_empty());
    }

    #[test]
    fn a_full_archive_says_so() {
        let mut zip = StoredZip::new(io::sink());
        for index in 0..MAX_ARCHIVE_FRAMES {
            zip.add(&index.to_string(), b"").unwrap();
        }
        assert!(matches!(zip.add("one more", b""), Err(ArchiveError::Full)));
    }
}
