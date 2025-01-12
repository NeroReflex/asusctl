//! Utils for writing to the `Slash` USB device
//!
//! Use of the device requires a few steps:
//! 1. Initialise the device by writing the two packets from
//! `get_init_packets()` 2. Write data from `SLashPacketType`
//! 3. Write the packet from `get_flush_packet()`, which tells the device to
//! display the data from step 2
//!
//! Step 1 needs to be applied only on fresh system boot.

use dmi_id::DMIID;

#[cfg(feature = "dbus")]
use crate::error::SlashError;
use crate::{SlashMode, SlashType};

const PACKET_SIZE: usize = 32;
const REPORT_ID_193B: u8 = 0x5e;
const REPORT_ID_19B6: u8 = 0x5d;

pub const VENDOR_ID: u16 = 0x0b05;

pub const PROD_ID1: u16 = 0x193b;
pub const PROD_ID1_STR: &str = "193B";
pub const PROD_ID2: u16 = 0x19b6;
pub const PROD_ID2_STR: &str = "19B6";

pub type SlashUsbPacket = [u8; PACKET_SIZE];

/// `get_anime_type` is very broad, matching on part of the laptop board name
/// only. For this reason `find_node()` must be used also to verify if the USB
/// device is available.
///
/// The currently known USB device is `193B`.
#[inline]
pub fn get_slash_type() -> SlashType {
    let dmi = DMIID::new()
        .map_err(|_| SlashError::NoDevice)
        .unwrap_or_default();
    let board_name = dmi.board_name;

    if board_name.contains("GA403") {
        SlashType::GA403
    } else if board_name.contains("GA605") {
        SlashType::GA605
    } else if board_name.contains("GU605") {
        SlashType::GU605
    } else {
        SlashType::Unsupported
    }
}

pub const fn report_id(slash_type: SlashType) -> u8 {
    match slash_type {
        SlashType::GA403 => REPORT_ID_193B,
        SlashType::GA605 => REPORT_ID_19B6,
        SlashType::GU605 => REPORT_ID_193B,
        SlashType::Unsupported => REPORT_ID_19B6
    }
}

/// Get the two device initialization packets. These are required for device
/// start after the laptop boots.
#[inline]
pub fn pkts_for_init(slash_type: SlashType) -> [SlashUsbPacket; 2] {
    let report_id = report_id(slash_type);

    let mut pkt1 = [0; PACKET_SIZE];
    pkt1[0] = report_id;
    pkt1[1] = 0xd7;
    pkt1[2] = 0x00;
    pkt1[3] = 0x00;
    pkt1[4] = 0x01;
    pkt1[5] = 0xac;

    let mut pkt2 = [0; PACKET_SIZE];
    pkt2[0] = report_id;
    pkt2[1] = 0xd2;
    pkt2[2] = 0x02;
    pkt2[3] = 0x01;
    pkt2[4] = 0x08;
    pkt2[5] = 0xab;

    [
        pkt1, pkt2
    ]
}

#[inline]
pub const fn pkt_save(slash_type: SlashType) -> SlashUsbPacket {
    let mut pkt = [0; PACKET_SIZE];
    pkt[0] = report_id(slash_type);
    pkt[1] = 0xd4;
    pkt[2] = 0x00;
    pkt[3] = 0x00;
    pkt[4] = 0x01;
    pkt[5] = 0xab;

    pkt
}

#[inline]
pub const fn pkt_set_mode(slash_type: SlashType, mode: SlashMode) -> [SlashUsbPacket; 2] {
    let report_id = report_id(slash_type);
    let mut pkt1 = [0; PACKET_SIZE];
    pkt1[0] = report_id;
    pkt1[1] = 0xd2;
    pkt1[2] = 0x03;
    pkt1[3] = 0x00;
    pkt1[4] = 0x0c;

    let mut pkt2 = [0; PACKET_SIZE];
    pkt2[0] = report_id;
    pkt2[1] = 0xd3;
    pkt2[2] = 0x04;
    pkt2[3] = 0x00;
    pkt2[4] = 0x0c;
    pkt2[5] = 0x01;
    pkt2[6] = mode as u8;
    pkt2[7] = 0x02;
    pkt2[8] = 0x19; // difference, GA605 = 0x10
    pkt2[9] = 0x03;
    pkt2[10] = 0x13;
    pkt2[11] = 0x04;
    pkt2[12] = 0x11;
    pkt2[13] = 0x05;
    pkt2[14] = 0x12;
    pkt2[15] = 0x06;
    pkt2[16] = 0x13;

    [
        pkt1, pkt2
    ]
}

pub const fn get_options_packet(
    slash_type: SlashType,
    enabled: bool,
    brightness: u8,
    interval: u8
) -> [u8; 13] {
    let typ = report_id(slash_type);
    let status = enabled as u8;
    [
        typ, 0xd3, 0x03, 0x01, 0x08, 0xab, 0xff, 0x01, status, 0x06, brightness, 0xff, interval
    ]
}

pub const fn get_boot_packet(slash_type: SlashType, enabled: bool) -> [u8; 12] {
    let typ = report_id(slash_type);
    let status = enabled as u8;
    [
        typ, 0xd3, 0x03, 0x01, 0x08, 0xa0, 0x04, 0xff, status, 0x01, 0xff, 0x00
    ]
}

pub const fn get_sleep_packet(slash_type: SlashType, enabled: bool) -> [u8; 12] {
    let typ = report_id(slash_type);
    let status = (!enabled) as u8;
    [
        typ, 0xd3, 0x03, 0x01, 0x08, 0xa1, 0x00, 0xff, status, 0x02, 0xff, 0xff
    ]
}

pub const fn get_low_battery_packet(slash_type: SlashType, enabled: bool) -> [u8; 12] {
    let typ = report_id(slash_type);
    let status = enabled as u8;
    [
        typ, 0xd3, 0x03, 0x01, 0x08, 0xa2, 0x01, 0xff, status, 0x02, 0xff, 0xff
    ]
}

pub const fn get_shutdown_packet(slash_type: SlashType, enabled: bool) -> [u8; 12] {
    let typ = report_id(slash_type);
    let status = enabled as u8;
    [
        typ, 0xd3, 0x03, 0x01, 0x08, 0xa4, 0x05, 0xff, status, 0x01, 0xff, 0x00
    ]
}

pub const fn get_battery_saver_packet(slash_type: SlashType, enabled: bool) -> [u8; 6] {
    let typ = report_id(slash_type);
    let status = if enabled { 0x00 } else { 0x80 };
    [
        typ, 0xd8, 0x01, 0x00, 0x01, status
    ]
}

pub const fn get_lid_closed_packet(slash_type: SlashType, enabled: bool) -> [u8; 7] {
    let typ = report_id(slash_type);
    let status = if enabled { 0x00 } else { 0x80 };
    [
        typ, 0xd8, 0x00, 0x00, 0x02, 0xa5, status
    ]
}
