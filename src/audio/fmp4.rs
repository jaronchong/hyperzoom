//! Audio-only fragmented MP4 (fMP4) writer.
//!
//! Writes a crash-safe fragmented MP4 file:
//! - Init segment: ftyp + moov (with mvex for fragmented playback)
//! - Fragments: moof + mdat pairs (~1 second each)
//! - Finalization: standard moov at EOF for non-fMP4-aware players
//!
//! Each fragment is flushed to disk, so a crash loses at most ~1 second.

use std::io::{Seek, SeekFrom, Write};

const TIMESCALE: u32 = 48_000;
const AAC_FRAME_DURATION: u32 = 1024; // samples per AAC-LC frame

/// Info tracked per AAC frame for finalization moov.
struct SampleInfo {
    file_offset: u64,
    size: u32,
    duration: u32,
}

/// Accumulates frames for the current fragment before flushing.
struct PendingFrame {
    data: Vec<u8>,
    duration: u32,
}

pub struct FragmentedMp4Writer<W: Write + Seek> {
    writer: W,
    /// AudioSpecificConfig for esds box.
    asc: Vec<u8>,
    /// Fragment sequence number (1-based).
    seq_num: u32,
    /// Running decode timestamp in timescale units.
    base_decode_time: u64,
    /// Frames accumulated for the current (unflushed) fragment.
    pending: Vec<PendingFrame>,
    /// All sample info for finalization moov.
    samples: Vec<SampleInfo>,
}

impl<W: Write + Seek> FragmentedMp4Writer<W> {
    /// Create a new writer and immediately write the init segment (ftyp + moov).
    pub fn new(mut writer: W, audio_specific_config: &[u8]) -> Result<Self, String> {
        let asc = audio_specific_config.to_vec();
        write_ftyp(&mut writer)?;
        write_init_moov(&mut writer, &asc)?;

        Ok(Self {
            writer,
            asc,
            seq_num: 0,
            base_decode_time: 0,
            pending: Vec::new(),
            samples: Vec::new(),
        })
    }

    /// Add one raw AAC frame to the current fragment.
    pub fn push_frame(&mut self, aac_data: &[u8]) {
        self.pending.push(PendingFrame {
            data: aac_data.to_vec(),
            duration: AAC_FRAME_DURATION,
        });
    }

    /// Number of pending (unflushed) frames.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Flush the current fragment to disk as a moof+mdat pair.
    /// No-op if there are no pending frames.
    pub fn flush_fragment(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }

        self.seq_num += 1;
        let seq = self.seq_num;
        let base_dt = self.base_decode_time;

        // Compute moof size to determine data_offset for trun
        let trun_entry_size = 8u32; // duration(4) + size(4) per sample
        let sample_count = self.pending.len() as u32;

        // trun: full box header(12) + sample_count(4) + data_offset(4) + entries
        let trun_size = 12 + 4 + 4 + sample_count * trun_entry_size;
        // tfdt: full box header(12) + base_decode_time_64(8)
        let tfdt_size = 20u32;
        // tfhd: full box header(12) + track_id(4)
        let tfhd_size = 16u32;
        // traf: box header(8) + tfhd + tfdt + trun
        let traf_size = 8 + tfhd_size + tfdt_size + trun_size;
        // mfhd: full box header(12) + sequence_number(4)
        let mfhd_size = 16u32;
        // moof: box header(8) + mfhd + traf
        let moof_size = 8 + mfhd_size + traf_size;

        let mdat_payload_size: u32 = self.pending.iter().map(|f| f.data.len() as u32).sum();
        let mdat_size = 8 + mdat_payload_size;

        // data_offset: from start of moof to start of mdat payload
        let data_offset = moof_size as i32 + 8; // +8 for mdat header

        // Write moof
        write_box_header(&mut self.writer, b"moof", moof_size)?;
        {
            // mfhd
            write_full_box_header(&mut self.writer, b"mfhd", mfhd_size, 0, 0)?;
            write_u32(&mut self.writer, seq)?;

            // traf
            write_box_header(&mut self.writer, b"traf", traf_size)?;
            {
                // tfhd — default-base-is-moof flag (0x020000)
                write_full_box_header(&mut self.writer, b"tfhd", tfhd_size, 0, 0x020000)?;
                write_u32(&mut self.writer, 1)?; // track_id

                // tfdt — version 1 (64-bit base_decode_time)
                write_full_box_header(&mut self.writer, b"tfdt", tfdt_size, 1, 0)?;
                write_u64(&mut self.writer, base_dt)?;

                // trun — flags: data-offset-present(0x01) | sample-duration(0x100) | sample-size(0x200)
                write_full_box_header(&mut self.writer, b"trun", trun_size, 0, 0x000301)?;
                write_u32(&mut self.writer, sample_count)?;
                write_i32(&mut self.writer, data_offset)?;
                for frame in &self.pending {
                    write_u32(&mut self.writer, frame.duration)?;
                    write_u32(&mut self.writer, frame.data.len() as u32)?;
                }
            }
        }

        // Write mdat
        write_box_header(&mut self.writer, b"mdat", mdat_size)?;
        let mdat_content_start = self
            .writer
            .stream_position()
            .map_err(|e| format!("seek error: {e}"))?;

        for (i, frame) in self.pending.iter().enumerate() {
            let offset = if i == 0 {
                mdat_content_start
            } else {
                self.writer
                    .stream_position()
                    .map_err(|e| format!("seek error: {e}"))?
            };
            self.writer
                .write_all(&frame.data)
                .map_err(|e| format!("write error: {e}"))?;
            self.samples.push(SampleInfo {
                file_offset: offset,
                size: frame.data.len() as u32,
                duration: frame.duration,
            });
        }

        // Update base_decode_time for next fragment
        let total_duration: u64 = self.pending.iter().map(|f| f.duration as u64).sum();
        self.base_decode_time += total_duration;

        self.pending.clear();

        // Flush to disk for crash safety
        self.writer.flush().map_err(|e| format!("flush error: {e}"))?;

        Ok(())
    }

    /// Write a standard moov at end of file for maximum player compatibility.
    /// Call this on clean shutdown after flush_fragment().
    pub fn finalize(mut self) -> Result<(), String> {
        // Flush any remaining frames first
        self.flush_fragment()?;

        if self.samples.is_empty() {
            return Ok(());
        }

        write_final_moov(&mut self.writer, &self.asc, &self.samples)?;
        self.writer.flush().map_err(|e| format!("flush error: {e}"))?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Box writing helpers
// ---------------------------------------------------------------------------

fn write_box_header<W: Write>(w: &mut W, box_type: &[u8; 4], size: u32) -> Result<(), String> {
    write_u32(w, size)?;
    w.write_all(box_type)
        .map_err(|e| format!("write error: {e}"))
}

fn write_full_box_header<W: Write>(
    w: &mut W,
    box_type: &[u8; 4],
    size: u32,
    version: u8,
    flags: u32,
) -> Result<(), String> {
    write_box_header(w, box_type, size)?;
    let vf = ((version as u32) << 24) | (flags & 0x00FFFFFF);
    write_u32(w, vf)
}

fn write_u8<W: Write>(w: &mut W, v: u8) -> Result<(), String> {
    w.write_all(&[v]).map_err(|e| format!("write error: {e}"))
}

fn write_u16<W: Write>(w: &mut W, v: u16) -> Result<(), String> {
    w.write_all(&v.to_be_bytes())
        .map_err(|e| format!("write error: {e}"))
}

fn write_u32<W: Write>(w: &mut W, v: u32) -> Result<(), String> {
    w.write_all(&v.to_be_bytes())
        .map_err(|e| format!("write error: {e}"))
}

fn write_i32<W: Write>(w: &mut W, v: i32) -> Result<(), String> {
    w.write_all(&v.to_be_bytes())
        .map_err(|e| format!("write error: {e}"))
}

fn write_u64<W: Write>(w: &mut W, v: u64) -> Result<(), String> {
    w.write_all(&v.to_be_bytes())
        .map_err(|e| format!("write error: {e}"))
}

fn write_zeros<W: Write>(w: &mut W, count: usize) -> Result<(), String> {
    for _ in 0..count {
        write_u8(w, 0)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ftyp box
// ---------------------------------------------------------------------------

fn write_ftyp<W: Write>(w: &mut W) -> Result<(), String> {
    // ftyp: major_brand(4) + minor_version(4) + compatible_brands(12) = 20 payload + 8 header
    let size = 8 + 4 + 4 + 12;
    write_box_header(w, b"ftyp", size)?;
    w.write_all(b"isom").map_err(|e| format!("write error: {e}"))?; // major brand
    write_u32(w, 0x200)?; // minor version
    w.write_all(b"isom").map_err(|e| format!("write error: {e}"))?; // compatible
    w.write_all(b"iso5").map_err(|e| format!("write error: {e}"))?;
    w.write_all(b"mp41").map_err(|e| format!("write error: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Init moov (for fragmented playback)
// ---------------------------------------------------------------------------

fn write_init_moov<W: Write + Seek>(w: &mut W, asc: &[u8]) -> Result<(), String> {
    // We use a size-patching approach: write placeholder, fill contents, patch size
    let moov_start = box_start_placeholder(w, b"moov")?;
    {
        write_mvhd(w)?;
        write_trak(w, asc)?;
        write_mvex(w)?;
    }
    patch_box_size(w, moov_start)
}

fn write_mvhd<W: Write + Seek>(w: &mut W) -> Result<(), String> {
    // mvhd: version 0, 108 bytes total
    let size = 108u32;
    write_full_box_header(w, b"mvhd", size, 0, 0)?;
    write_u32(w, 0)?; // creation_time
    write_u32(w, 0)?; // modification_time
    write_u32(w, TIMESCALE)?; // timescale
    write_u32(w, 0)?; // duration (unknown for fragmented)
    write_u32(w, 0x00010000)?; // rate = 1.0 (fixed-point 16.16)
    write_u16(w, 0x0100)?; // volume = 1.0 (fixed-point 8.8)
    write_zeros(w, 10)?; // reserved
    // Unity matrix (9 * 4 = 36 bytes)
    for &v in &[
        0x00010000u32,
        0,
        0,
        0,
        0x00010000,
        0,
        0,
        0,
        0x40000000,
    ] {
        write_u32(w, v)?;
    }
    write_zeros(w, 24)?; // pre_defined[6]
    write_u32(w, 2)?; // next_track_ID
    Ok(())
}

fn write_trak<W: Write + Seek>(w: &mut W, asc: &[u8]) -> Result<(), String> {
    let trak_start = box_start_placeholder(w, b"trak")?;
    {
        write_tkhd(w)?;
        write_mdia(w, asc)?;
    }
    patch_box_size(w, trak_start)
}

fn write_tkhd<W: Write + Seek>(w: &mut W) -> Result<(), String> {
    // tkhd version 0: 92 bytes total
    let size = 92u32;
    // flags: track_enabled(0x01) | track_in_movie(0x02)
    write_full_box_header(w, b"tkhd", size, 0, 0x03)?;
    write_u32(w, 0)?; // creation_time
    write_u32(w, 0)?; // modification_time
    write_u32(w, 1)?; // track_ID
    write_u32(w, 0)?; // reserved
    write_u32(w, 0)?; // duration (unknown)
    write_zeros(w, 8)?; // reserved[2]
    write_u16(w, 0)?; // layer
    write_u16(w, 0)?; // alternate_group
    write_u16(w, 0x0100)?; // volume = 1.0 (audio track)
    write_u16(w, 0)?; // reserved
    // Unity matrix
    for &v in &[
        0x00010000u32,
        0,
        0,
        0,
        0x00010000,
        0,
        0,
        0,
        0x40000000,
    ] {
        write_u32(w, v)?;
    }
    write_u32(w, 0)?; // width (audio = 0)
    write_u32(w, 0)?; // height (audio = 0)
    Ok(())
}

fn write_mdia<W: Write + Seek>(w: &mut W, asc: &[u8]) -> Result<(), String> {
    let mdia_start = box_start_placeholder(w, b"mdia")?;
    {
        write_mdhd(w)?;
        write_hdlr(w)?;
        write_minf(w, asc)?;
    }
    patch_box_size(w, mdia_start)
}

fn write_mdhd<W: Write + Seek>(w: &mut W) -> Result<(), String> {
    let size = 32u32;
    write_full_box_header(w, b"mdhd", size, 0, 0)?;
    write_u32(w, 0)?; // creation_time
    write_u32(w, 0)?; // modification_time
    write_u32(w, TIMESCALE)?; // timescale
    write_u32(w, 0)?; // duration (unknown)
    write_u16(w, 0x55C4)?; // language: undetermined
    write_u16(w, 0)?; // pre_defined
    Ok(())
}

fn write_hdlr<W: Write + Seek>(w: &mut W) -> Result<(), String> {
    let name = b"SoundHandler\0";
    let size = 8 + 4 + 4 + 12 + name.len() as u32; // box(8) + version_flags(4) + pre_defined(4) + handler+reserved(12) + name
    write_full_box_header(w, b"hdlr", size, 0, 0)?;
    write_u32(w, 0)?; // pre_defined
    w.write_all(b"soun").map_err(|e| format!("write error: {e}"))?; // handler_type
    write_zeros(w, 12)?; // reserved[3]
    w.write_all(name).map_err(|e| format!("write error: {e}"))?;
    Ok(())
}

fn write_minf<W: Write + Seek>(w: &mut W, asc: &[u8]) -> Result<(), String> {
    let minf_start = box_start_placeholder(w, b"minf")?;
    {
        write_smhd(w)?;
        write_dinf(w)?;
        write_stbl(w, asc)?;
    }
    patch_box_size(w, minf_start)
}

fn write_smhd<W: Write + Seek>(w: &mut W) -> Result<(), String> {
    let size = 16u32;
    write_full_box_header(w, b"smhd", size, 0, 0)?;
    write_u16(w, 0)?; // balance
    write_u16(w, 0)?; // reserved
    Ok(())
}

fn write_dinf<W: Write + Seek>(w: &mut W) -> Result<(), String> {
    // dinf → dref → url
    let url_size = 12u32; // full box with self-contained flag
    let dref_size = 8 + 4 + 4 + url_size; // box(8) + version_flags(4) + entry_count(4) + url
    let dinf_size = 8 + dref_size;
    write_box_header(w, b"dinf", dinf_size)?;
    write_full_box_header(w, b"dref", dref_size, 0, 0)?;
    write_u32(w, 1)?; // entry_count
    // url with self-contained flag (0x01)
    write_full_box_header(w, b"url ", url_size, 0, 0x01)?;
    Ok(())
}

fn write_stbl<W: Write + Seek>(w: &mut W, asc: &[u8]) -> Result<(), String> {
    let stbl_start = box_start_placeholder(w, b"stbl")?;
    {
        write_stsd(w, asc)?;
        // Empty required tables for init segment
        write_empty_stts(w)?;
        write_empty_stsc(w)?;
        write_empty_stsz(w)?;
        write_empty_stco(w)?;
    }
    patch_box_size(w, stbl_start)
}

fn write_stsd<W: Write + Seek>(w: &mut W, asc: &[u8]) -> Result<(), String> {
    let stsd_start = box_start_placeholder_full(w, b"stsd", 0, 0)?;
    write_u32(w, 1)?; // entry_count

    // mp4a sample entry
    let esds_inner = build_esds_contents(asc);
    let esds_size = 12 + esds_inner.len() as u32; // full box header + contents
    let mp4a_size = 8 + 6 + 2 + 8 + 2 + 2 + 4 + 2 + 2 + esds_size;
    write_box_header(w, b"mp4a", mp4a_size)?;
    write_zeros(w, 6)?; // reserved
    write_u16(w, 1)?; // data_reference_index
    write_zeros(w, 8)?; // reserved
    write_u16(w, 1)?; // channel_count (mono)
    write_u16(w, 16)?; // sample_size (bits)
    write_u32(w, 0)?; // pre_defined + reserved
    write_u16(w, (TIMESCALE >> 16) as u16)?; // high part of sample rate (fixed 16.16)
    write_u16(w, 0)?; // low part

    // esds box
    write_full_box_header(w, b"esds", esds_size, 0, 0)?;
    w.write_all(&esds_inner)
        .map_err(|e| format!("write error: {e}"))?;

    patch_box_size(w, stsd_start)
}

fn build_esds_contents(asc: &[u8]) -> Vec<u8> {
    // ES_Descriptor
    let mut buf = Vec::new();
    // tag 0x03 (ES_Descriptor)
    let dec_config_len = 13 + 2 + asc.len(); // DecoderConfigDescriptor
    let sl_config_len = 1; // SLConfigDescriptor
    let es_desc_len = 3 + (2 + dec_config_len) + (2 + sl_config_len);
    buf.push(0x03); // ES_Descriptor tag
    buf.push(es_desc_len as u8);
    buf.extend_from_slice(&[0x00, 0x01]); // ES_ID = 1
    buf.push(0x00); // stream priority

    // DecoderConfigDescriptor (tag 0x04)
    buf.push(0x04);
    buf.push(dec_config_len as u8);
    buf.push(0x40); // objectTypeIndication: Audio ISO/IEC 14496-3
    buf.push(0x15); // streamType: audio(5) << 2 | upstream(0) << 1 | 1
    buf.extend_from_slice(&[0x00, 0x00, 0x00]); // bufferSizeDB (24 bits)
    buf.extend_from_slice(&BITRATE.to_be_bytes()); // maxBitrate
    buf.extend_from_slice(&BITRATE.to_be_bytes()); // avgBitrate

    // DecoderSpecificInfo (tag 0x05)
    buf.push(0x05);
    buf.push(asc.len() as u8);
    buf.extend_from_slice(asc);

    // SLConfigDescriptor (tag 0x06)
    buf.push(0x06);
    buf.push(sl_config_len as u8);
    buf.push(0x02); // predefined = MP4

    buf
}

fn write_empty_stts<W: Write>(w: &mut W) -> Result<(), String> {
    write_full_box_header(w, b"stts", 16, 0, 0)?;
    write_u32(w, 0) // entry_count
}

fn write_empty_stsc<W: Write>(w: &mut W) -> Result<(), String> {
    write_full_box_header(w, b"stsc", 16, 0, 0)?;
    write_u32(w, 0)
}

fn write_empty_stsz<W: Write>(w: &mut W) -> Result<(), String> {
    write_full_box_header(w, b"stsz", 20, 0, 0)?;
    write_u32(w, 0)?; // sample_size (0 = variable)
    write_u32(w, 0) // sample_count
}

fn write_empty_stco<W: Write>(w: &mut W) -> Result<(), String> {
    write_full_box_header(w, b"stco", 16, 0, 0)?;
    write_u32(w, 0)
}

fn write_mvex<W: Write + Seek>(w: &mut W) -> Result<(), String> {
    let mvex_size = 8 + 32; // mvex header + trex
    write_box_header(w, b"mvex", mvex_size)?;
    // trex
    let trex_size = 32u32;
    write_full_box_header(w, b"trex", trex_size, 0, 0)?;
    write_u32(w, 1)?; // track_ID
    write_u32(w, 1)?; // default_sample_description_index
    write_u32(w, AAC_FRAME_DURATION)?; // default_sample_duration
    write_u32(w, 0)?; // default_sample_size
    write_u32(w, 0)?; // default_sample_flags
    Ok(())
}

// ---------------------------------------------------------------------------
// Finalization moov (standard sample tables)
// ---------------------------------------------------------------------------

fn write_final_moov<W: Write + Seek>(
    w: &mut W,
    asc: &[u8],
    samples: &[SampleInfo],
) -> Result<(), String> {
    let total_duration: u64 = samples.iter().map(|s| s.duration as u64).sum();

    let moov_start = box_start_placeholder(w, b"moov")?;
    {
        // mvhd with known duration
        write_final_mvhd(w, total_duration as u32)?;
        // trak with full sample tables
        write_final_trak(w, asc, samples, total_duration as u32)?;
    }
    patch_box_size(w, moov_start)
}

fn write_final_mvhd<W: Write + Seek>(w: &mut W, duration: u32) -> Result<(), String> {
    let size = 108u32;
    write_full_box_header(w, b"mvhd", size, 0, 0)?;
    write_u32(w, 0)?; // creation_time
    write_u32(w, 0)?; // modification_time
    write_u32(w, TIMESCALE)?;
    write_u32(w, duration)?;
    write_u32(w, 0x00010000)?; // rate
    write_u16(w, 0x0100)?; // volume
    write_zeros(w, 10)?;
    for &v in &[
        0x00010000u32,
        0,
        0,
        0,
        0x00010000,
        0,
        0,
        0,
        0x40000000,
    ] {
        write_u32(w, v)?;
    }
    write_zeros(w, 24)?;
    write_u32(w, 2)?; // next_track_ID
    Ok(())
}

fn write_final_trak<W: Write + Seek>(
    w: &mut W,
    asc: &[u8],
    samples: &[SampleInfo],
    duration: u32,
) -> Result<(), String> {
    let trak_start = box_start_placeholder(w, b"trak")?;
    {
        write_final_tkhd(w, duration)?;
        write_final_mdia(w, asc, samples, duration)?;
    }
    patch_box_size(w, trak_start)
}

fn write_final_tkhd<W: Write + Seek>(w: &mut W, duration: u32) -> Result<(), String> {
    let size = 92u32;
    write_full_box_header(w, b"tkhd", size, 0, 0x03)?;
    write_u32(w, 0)?; // creation_time
    write_u32(w, 0)?; // modification_time
    write_u32(w, 1)?; // track_ID
    write_u32(w, 0)?; // reserved
    write_u32(w, duration)?;
    write_zeros(w, 8)?;
    write_u16(w, 0)?;
    write_u16(w, 0)?;
    write_u16(w, 0x0100)?; // volume
    write_u16(w, 0)?;
    for &v in &[
        0x00010000u32,
        0,
        0,
        0,
        0x00010000,
        0,
        0,
        0,
        0x40000000,
    ] {
        write_u32(w, v)?;
    }
    write_u32(w, 0)?;
    write_u32(w, 0)?;
    Ok(())
}

fn write_final_mdia<W: Write + Seek>(
    w: &mut W,
    asc: &[u8],
    samples: &[SampleInfo],
    duration: u32,
) -> Result<(), String> {
    let mdia_start = box_start_placeholder(w, b"mdia")?;
    {
        // mdhd with duration
        let size = 32u32;
        write_full_box_header(w, b"mdhd", size, 0, 0)?;
        write_u32(w, 0)?;
        write_u32(w, 0)?;
        write_u32(w, TIMESCALE)?;
        write_u32(w, duration)?;
        write_u16(w, 0x55C4)?;
        write_u16(w, 0)?;

        write_hdlr(w)?;

        let minf_start = box_start_placeholder(w, b"minf")?;
        {
            write_smhd(w)?;
            write_dinf(w)?;
            write_final_stbl(w, asc, samples)?;
        }
        patch_box_size(w, minf_start)?;
    }
    patch_box_size(w, mdia_start)
}

fn write_final_stbl<W: Write + Seek>(
    w: &mut W,
    asc: &[u8],
    samples: &[SampleInfo],
) -> Result<(), String> {
    let stbl_start = box_start_placeholder(w, b"stbl")?;
    {
        write_stsd(w, asc)?;

        // stts — run-length encoded durations
        // All frames have the same duration (1024), so single entry
        let stts_size = 16 + 8; // header + 1 entry (sample_count, sample_delta)
        write_full_box_header(w, b"stts", stts_size, 0, 0)?;
        write_u32(w, 1)?; // entry_count
        write_u32(w, samples.len() as u32)?; // sample_count
        write_u32(w, AAC_FRAME_DURATION)?; // sample_delta

        // stsz — per-sample sizes
        let stsz_size = 20 + 4 * samples.len() as u32;
        write_full_box_header(w, b"stsz", stsz_size, 0, 0)?;
        write_u32(w, 0)?; // sample_size = 0 (variable)
        write_u32(w, samples.len() as u32)?;
        for s in samples {
            write_u32(w, s.size)?;
        }

        // stsc — one chunk per sample (simple approach)
        let stsc_size = 16 + 12; // header + 1 entry
        write_full_box_header(w, b"stsc", stsc_size, 0, 0)?;
        write_u32(w, 1)?; // entry_count
        write_u32(w, 1)?; // first_chunk
        write_u32(w, 1)?; // samples_per_chunk
        write_u32(w, 1)?; // sample_description_index

        // stco / co64 — per-sample chunk offsets
        // Use co64 (64-bit) since file offsets may exceed 4GB for long recordings
        let co64_size = 16 + 8 * samples.len() as u32;
        write_full_box_header(w, b"co64", co64_size, 0, 0)?;
        write_u32(w, samples.len() as u32)?;
        for s in samples {
            write_u64(w, s.file_offset)?;
        }
    }
    patch_box_size(w, stbl_start)
}

// ---------------------------------------------------------------------------
// Size patching helpers
// ---------------------------------------------------------------------------

/// Write a placeholder box header and return the start position.
fn box_start_placeholder<W: Write + Seek>(w: &mut W, box_type: &[u8; 4]) -> Result<u64, String> {
    let pos = w
        .stream_position()
        .map_err(|e| format!("seek error: {e}"))?;
    write_box_header(w, box_type, 0)?; // placeholder size
    Ok(pos)
}

/// Write a placeholder full box header and return the start position.
fn box_start_placeholder_full<W: Write + Seek>(
    w: &mut W,
    box_type: &[u8; 4],
    version: u8,
    flags: u32,
) -> Result<u64, String> {
    let pos = w
        .stream_position()
        .map_err(|e| format!("seek error: {e}"))?;
    write_full_box_header(w, box_type, 0, version, flags)?; // placeholder size
    Ok(pos)
}

/// Patch the box size at the given start position.
fn patch_box_size<W: Write + Seek>(w: &mut W, start: u64) -> Result<(), String> {
    let end = w
        .stream_position()
        .map_err(|e| format!("seek error: {e}"))?;
    let size = (end - start) as u32;
    w.seek(SeekFrom::Start(start))
        .map_err(|e| format!("seek error: {e}"))?;
    write_u32(w, size)?;
    w.seek(SeekFrom::Start(end))
        .map_err(|e| format!("seek error: {e}"))?;
    Ok(())
}

const BITRATE: u32 = 192_000;
