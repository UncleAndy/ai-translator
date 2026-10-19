// @generated from schema/ntgcalls.ntl -- DO NOT EDIT
#![allow(non_camel_case_types, non_upper_case_globals, non_snake_case, dead_code)]

use std::os::raw::{c_char, c_int, c_void};

#[repr(C)]
pub struct ntg_bytes {
    pub data: *mut u8,
    pub len: usize,
}

pub type ntg_result = c_int;
pub const NTG_OK: ntg_result = 0;
pub const NTG_ERR_UNKNOWN: ntg_result = -1;
pub const NTG_ERR_NULL_POINTER: ntg_result = -2;
pub const NTG_ERR_RTC: ntg_result = -100;
pub const NTG_ERR_SDP_PARSE: ntg_result = -101;
pub const NTG_ERR_TRANSPORT_PARSE: ntg_result = -102;
pub const NTG_ERR_RTMP_STREAMING_UNSUPPORTED: ntg_result = -103;
pub const NTG_ERR_CONNECTION: ntg_result = -200;
pub const NTG_ERR_CONNECTION_NOT_FOUND: ntg_result = -201;
pub const NTG_ERR_CONNECTION_ERROR: ntg_result = -202;
pub const NTG_ERR_CRYPTO_ERROR: ntg_result = -203;
pub const NTG_ERR_TELEGRAM_SERVER_ERROR: ntg_result = -204;
pub const NTG_ERR_RTC_CONNECTION_NEEDED: ntg_result = -205;
pub const NTG_ERR_INVALID_PARAMS: ntg_result = -206;
pub const NTG_ERR_SIGNALING: ntg_result = -300;
pub const NTG_ERR_SIGNALING_ERROR: ntg_result = -301;
pub const NTG_ERR_SIGNALING_UNSUPPORTED: ntg_result = -302;
pub const NTG_ERR_MEDIA: ntg_result = -400;
pub const NTG_ERR_FILE_ERROR: ntg_result = -401;
pub const NTG_ERR_FFMPEG_ERROR: ntg_result = -402;
pub const NTG_ERR_SHELL_ERROR: ntg_result = -403;
pub const NTG_ERR_MEDIA_DEVICE_ERROR: ntg_result = -404;

pub type ntg_log_level = c_int;
pub const NTG_LOG_DEBUG: ntg_log_level = 1;
pub const NTG_LOG_INFO: ntg_log_level = 2;
pub const NTG_LOG_WARNING: ntg_log_level = 4;
pub const NTG_LOG_ERROR: ntg_log_level = 8;

pub type ntg_log_source = c_int;
pub const NTG_LOG_SOURCE_WEBRTC: ntg_log_source = 1;
pub const NTG_LOG_SOURCE_SELF: ntg_log_source = 2;

#[repr(C)]
pub struct ntg_log_message {
    pub level: ntg_log_level,
    pub source: ntg_log_source,
    pub file: *const c_char,
    pub line: u32,
    pub message: *const c_char,
}

pub type ntg_log_cb = Option<unsafe extern "C" fn(message: ntg_log_message, user_data: *mut c_void)>;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_media_source {
    NTG_MEDIA_SOURCE_UNKNOWN,
    NTG_MEDIA_SOURCE_FILE,
    NTG_MEDIA_SOURCE_SHELL,
    NTG_MEDIA_SOURCE_FFMPEG,
    NTG_MEDIA_SOURCE_DEVICE,
    NTG_MEDIA_SOURCE_DESKTOP,
    NTG_MEDIA_SOURCE_EXTERNAL,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_stream_type {
    NTG_STREAM_TYPE_AUDIO,
    NTG_STREAM_TYPE_VIDEO,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_stream_status {
    NTG_STREAM_STATUS_ACTIVE,
    NTG_STREAM_STATUS_PAUSED,
    NTG_STREAM_STATUS_IDLING,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_stream_mode {
    NTG_STREAM_MODE_CAPTURE,
    NTG_STREAM_MODE_PLAYBACK,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_stream_device {
    NTG_STREAM_DEVICE_MICROPHONE,
    NTG_STREAM_DEVICE_SPEAKER,
    NTG_STREAM_DEVICE_CAMERA,
    NTG_STREAM_DEVICE_SCREEN,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_connection_state {
    NTG_CONNECTION_STATE_CONNECTING,
    NTG_CONNECTION_STATE_CONNECTED,
    NTG_CONNECTION_STATE_FAILED,
    NTG_CONNECTION_STATE_TIMEOUT,
    NTG_CONNECTION_STATE_CLOSED,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_connection_kind {
    NTG_CONNECTION_KIND_NORMAL,
    NTG_CONNECTION_KIND_PRESENTATION,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_call_type {
    NTG_CALL_TYPE_GROUP,
    NTG_CALL_TYPE_OUTGOING,
    NTG_CALL_TYPE_INCOMING,
    NTG_CALL_TYPE_P2P,
    NTG_CALL_TYPE_CONFERENCE,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_media_segment_quality {
    NTG_MEDIA_SEGMENT_QUALITY_NONE,
    NTG_MEDIA_SEGMENT_QUALITY_THUMBNAIL,
    NTG_MEDIA_SEGMENT_QUALITY_MEDIUM,
    NTG_MEDIA_SEGMENT_QUALITY_FULL,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_media_segment_part_status {
    NTG_MEDIA_SEGMENT_PART_STATUS_NOT_READY,
    NTG_MEDIA_SEGMENT_PART_STATUS_RESYNC_NEEDED,
    NTG_MEDIA_SEGMENT_PART_STATUS_DOWNLOADING,
    NTG_MEDIA_SEGMENT_PART_STATUS_SUCCESS,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_connection_mode {
    NTG_CONNECTION_MODE_NONE,
    NTG_CONNECTION_MODE_RTC,
    NTG_CONNECTION_MODE_STREAM,
    NTG_CONNECTION_MODE_RTMP,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ntg_video_rotation {
    NTG_VIDEO_ROTATION_VIDEO_ROTATION_0,
    NTG_VIDEO_ROTATION_VIDEO_ROTATION_90,
    NTG_VIDEO_ROTATION_VIDEO_ROTATION_180,
    NTG_VIDEO_ROTATION_VIDEO_ROTATION_270,
}

#[repr(C)]
pub struct ntg_connection_info {
    pub state: ntg_connection_state,
    pub kind: ntg_connection_kind,
}

#[repr(C)]
pub struct ntg_remote_source {
    pub ssrc: u32,
    pub state: ntg_stream_status,
    pub device: ntg_stream_device,
}

#[repr(C)]
pub struct ntg_subchain_request {
    pub subchain: i32,
    pub height: i32,
    pub limit: i32,
}

#[repr(C)]
pub struct ntg_audio_description {
    pub media_source: ntg_media_source,
    pub sample_rate: u32,
    pub channel_count: u8,
    pub input: *mut c_char,
    pub keep_open: bool,
}

#[repr(C)]
pub struct ntg_call_info {
    pub playback: ntg_stream_status,
    pub capture: ntg_stream_status,
}

#[repr(C)]
pub struct ntg_device_info {
    pub name: *mut c_char,
    pub metadata: *mut c_char,
}

#[repr(C)]
pub struct ntg_media_devices {
    pub microphone: *mut ntg_device_info,
    pub microphone_len: usize,
    pub speaker: *mut ntg_device_info,
    pub speaker_len: usize,
    pub camera: *mut ntg_device_info,
    pub camera_len: usize,
    pub screen: *mut ntg_device_info,
    pub screen_len: usize,
}

#[repr(C)]
pub struct ntg_media_state {
    pub muted: bool,
    pub video_paused: bool,
    pub video_stopped: bool,
    pub presentation_paused: bool,
    pub presentation_stopped: bool,
}

#[repr(C)]
pub struct ntg_video_description {
    pub media_source: ntg_media_source,
    pub width: i16,
    pub height: i16,
    pub fps: u8,
    pub input: *mut c_char,
    pub keep_open: bool,
}

#[repr(C)]
pub struct ntg_media_description {
    pub microphone: *mut ntg_audio_description,
    pub speaker: *mut ntg_audio_description,
    pub camera: *mut ntg_video_description,
    pub screen: *mut ntg_video_description,
}

#[repr(C)]
pub struct ntg_frame_data {
    pub absolute_capture_timestamp_ms: i64,
    pub rotation: ntg_video_rotation,
    pub width: u16,
    pub height: u16,
}

#[repr(C)]
pub struct ntg_participants_request {
    pub ssrcs: *mut u32,
    pub ssrcs_len: usize,
}

#[repr(C)]
pub struct ntg_ssrc_group {
    pub semantics: *mut c_char,
    pub ssrcs: *mut u32,
    pub ssrcs_len: usize,
}

#[repr(C)]
pub struct ntg_frame {
    pub ssrc: i64,
    pub data: *mut u8,
    pub data_len: usize,
    pub frame_data: ntg_frame_data,
}

#[repr(C)]
pub struct ntg_segment_part_request {
    pub segment_id: i64,
    pub part_id: i32,
    pub limit: i32,
    pub timestamp: i64,
    pub quality_update: bool,
    pub channel_id: i32,
    pub quality: ntg_media_segment_quality,
}

#[repr(C)]
pub struct ntg_ssrc_mapping {
    pub user_id: i64,
    pub ssrc: i32,
}

#[repr(C)]
pub struct ntg_auth_params {
    pub key_fingerprint: i64,
    pub g_a_or_b: *mut u8,
    pub g_a_or_b_len: usize,
}

#[repr(C)]
pub struct ntg_conference_join_params {
    pub payload: *mut c_char,
    pub public_key: *mut u8,
    pub public_key_len: usize,
    pub block: *mut u8,
    pub block_len: usize,
}

#[repr(C)]
pub struct ntg_dh_config {
    pub g: i32,
    pub p: *mut u8,
    pub p_len: usize,
    pub random: *mut u8,
    pub random_len: usize,
}

#[repr(C)]
pub struct ntg_protocol {
    pub min_layer: i32,
    pub max_layer: i32,
    pub udp_p2p: bool,
    pub udp_reflector: bool,
    pub library_versions: *mut *mut c_char,
    pub library_versions_len: usize,
}

#[repr(C)]
pub struct ntg_rtc_server {
    pub id: u64,
    pub ipv4: *mut c_char,
    pub ipv6: *mut c_char,
    pub port: u16,
    pub username: *mut c_char,
    pub password: *mut c_char,
    pub turn: bool,
    pub stun: bool,
    pub tcp: bool,
    pub peer_tag: *mut u8,
    pub peer_tag_len: usize,
}

#[repr(C)]
pub struct ntg_call_info_entry {
    pub key: i64,
    pub value: ntg_call_info,
}

#[repr(C)]
pub struct ntg_instance {
    _private: [u8; 0],
}

pub type ntg_upgrade_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    state: ntg_media_state,
    user_data: *mut c_void,
)>;

pub type ntg_stream_end_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    r#type: ntg_stream_type,
    device: ntg_stream_device,
    user_data: *mut c_void,
)>;

pub type ntg_connection_change_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    state: ntg_connection_info,
    user_data: *mut c_void,
)>;

pub type ntg_frames_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    mode: ntg_stream_mode,
    device: ntg_stream_device,
    frames: *const ntg_frame, frames_len: usize,
    user_data: *mut c_void,
)>;

pub type ntg_signaling_data_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    data: *const u8, data_len: usize,
    user_data: *mut c_void,
)>;

pub type ntg_remote_source_change_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    state: ntg_remote_source,
    user_data: *mut c_void,
)>;

pub type ntg_request_broadcast_part_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    request: ntg_segment_part_request,
    user_data: *mut c_void,
)>;

pub type ntg_request_broadcast_timestamp_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    user_data: *mut c_void,
)>;

pub type ntg_request_participants_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    request: ntg_participants_request,
    user_data: *mut c_void,
)>;

pub type ntg_outbound_block_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    block: *const u8, block_len: usize,
    user_data: *mut c_void,
)>;

pub type ntg_subchain_request_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    subchain_request: ntg_subchain_request,
    user_data: *mut c_void,
)>;

pub type ntg_update_emojis_callback_cb = Option<unsafe extern "C" fn(
    handle: *mut ntg_instance,
    chat_id: i64,
    emojis: *const c_char,
    user_data: *mut c_void,
)>;

extern "C" {
    pub fn ntg_get_version() -> *const c_char;
    pub fn ntg_last_error() -> *const c_char;
    pub fn ntg_set_log_callback(callback: ntg_log_cb, user_data: *mut c_void);
    pub fn ntg_instance_create() -> *mut ntg_instance;
    pub fn ntg_instance_destroy(handle: *mut ntg_instance);
    pub fn ntg_string_free(value: *mut c_char);
    pub fn ntg_bytes_free(value: *mut c_void);
    pub fn ntg_connection_info_free(value: *mut ntg_connection_info);
    pub fn ntg_remote_source_free(value: *mut ntg_remote_source);
    pub fn ntg_subchain_request_free(value: *mut ntg_subchain_request);
    pub fn ntg_audio_description_free(value: *mut ntg_audio_description);
    pub fn ntg_call_info_free(value: *mut ntg_call_info);
    pub fn ntg_device_info_free(value: *mut ntg_device_info);
    pub fn ntg_media_devices_free(value: *mut ntg_media_devices);
    pub fn ntg_media_state_free(value: *mut ntg_media_state);
    pub fn ntg_video_description_free(value: *mut ntg_video_description);
    pub fn ntg_media_description_free(value: *mut ntg_media_description);
    pub fn ntg_frame_data_free(value: *mut ntg_frame_data);
    pub fn ntg_participants_request_free(value: *mut ntg_participants_request);
    pub fn ntg_ssrc_group_free(value: *mut ntg_ssrc_group);
    pub fn ntg_frame_free(value: *mut ntg_frame);
    pub fn ntg_segment_part_request_free(value: *mut ntg_segment_part_request);
    pub fn ntg_ssrc_mapping_free(value: *mut ntg_ssrc_mapping);
    pub fn ntg_auth_params_free(value: *mut ntg_auth_params);
    pub fn ntg_conference_join_params_free(value: *mut ntg_conference_join_params);
    pub fn ntg_dh_config_free(value: *mut ntg_dh_config);
    pub fn ntg_protocol_free(value: *mut ntg_protocol);
    pub fn ntg_rtc_server_free(value: *mut ntg_rtc_server);
    pub fn ntg_call_info_entry_free(entries: *mut ntg_call_info_entry, len: usize);

    pub fn ntg_create_p2p_call(
        handle: *mut ntg_instance,
        user_id: i64,
    ) -> ntg_result;
    pub fn ntg_init_exchange(
        handle: *mut ntg_instance,
        user_id: i64,
        dh_config: ntg_dh_config,
        ga_hash: *const u8, ga_hash_len: usize,
        out: *mut *mut u8, out_len: *mut usize,
    ) -> ntg_result;
    pub fn ntg_exchange_keys(
        handle: *mut ntg_instance,
        user_id: i64,
        g_a_or_b: *const u8, g_a_or_b_len: usize,
        fingerprint: i64,
        out: *mut ntg_auth_params,
    ) -> ntg_result;
    pub fn ntg_skip_exchange(
        handle: *mut ntg_instance,
        user_id: i64,
        encryption_key: *const u8, encryption_key_len: usize,
        is_outgoing: bool,
    ) -> ntg_result;
    pub fn ntg_connect_p2p(
        handle: *mut ntg_instance,
        user_id: i64,
        servers: *const ntg_rtc_server, servers_len: usize,
        versions: *const *mut c_char, versions_len: usize,
        p2p_allowed: bool,
        custom_parameters: *const c_char,
    ) -> ntg_result;
    pub fn ntg_create_call(
        handle: *mut ntg_instance,
        chat_id: i64,
        out: *mut *mut c_char,
    ) -> ntg_result;
    pub fn ntg_init_presentation(
        handle: *mut ntg_instance,
        chat_id: i64,
        out: *mut *mut c_char,
    ) -> ntg_result;
    pub fn ntg_init_conference(
        handle: *mut ntg_instance,
        chat_id: i64,
        user_id: i64,
        last_block: *const u8, last_block_len: usize,
        out: *mut ntg_conference_join_params,
    ) -> ntg_result;
    pub fn ntg_connect(
        handle: *mut ntg_instance,
        chat_id: i64,
        params: *const c_char,
        is_presentation: bool,
    ) -> ntg_result;
    pub fn ntg_add_incoming_video(
        handle: *mut ntg_instance,
        chat_id: i64,
        user_id: i64,
        endpoint: *const c_char,
        ssrc_groups: *const ntg_ssrc_group, ssrc_groups_len: usize,
        out: *mut u32,
    ) -> ntg_result;
    pub fn ntg_remove_incoming_video(
        handle: *mut ntg_instance,
        chat_id: i64,
        endpoint: *const c_char,
        out: *mut bool,
    ) -> ntg_result;
    pub fn ntg_set_stream_sources(
        handle: *mut ntg_instance,
        chat_id: i64,
        mode: ntg_stream_mode,
        media: ntg_media_description,
    ) -> ntg_result;
    pub fn ntg_pause(
        handle: *mut ntg_instance,
        chat_id: i64,
        out: *mut bool,
    ) -> ntg_result;
    pub fn ntg_resume(
        handle: *mut ntg_instance,
        chat_id: i64,
        out: *mut bool,
    ) -> ntg_result;
    pub fn ntg_mute(
        handle: *mut ntg_instance,
        chat_id: i64,
        out: *mut bool,
    ) -> ntg_result;
    pub fn ntg_unmute(
        handle: *mut ntg_instance,
        chat_id: i64,
        out: *mut bool,
    ) -> ntg_result;
    pub fn ntg_stop(
        handle: *mut ntg_instance,
        chat_id: i64,
    ) -> ntg_result;
    pub fn ntg_stop_presentation(
        handle: *mut ntg_instance,
        chat_id: i64,
    ) -> ntg_result;
    pub fn ntg_get_emojis_fingerprint(
        handle: *mut ntg_instance,
        chat_id: i64,
        out: *mut *mut c_char,
    ) -> ntg_result;
    pub fn ntg_time(
        handle: *mut ntg_instance,
        chat_id: i64,
        mode: ntg_stream_mode,
        out: *mut u64,
    ) -> ntg_result;
    pub fn ntg_get_state(
        handle: *mut ntg_instance,
        chat_id: i64,
        out: *mut ntg_media_state,
    ) -> ntg_result;
    pub fn ntg_get_call_type(
        handle: *mut ntg_instance,
        chat_id: i64,
        out: *mut ntg_call_type,
    ) -> ntg_result;
    pub fn ntg_get_connection_mode(
        handle: *mut ntg_instance,
        chat_id: i64,
        out: *mut ntg_connection_mode,
    ) -> ntg_result;
    pub fn ntg_cpu_usage(
        handle: *mut ntg_instance,
        out: *mut f64,
    ) -> ntg_result;
    pub fn ntg_ping(
        out: *mut *mut c_char,
    ) -> ntg_result;
    pub fn ntg_get_media_devices(
        out: *mut ntg_media_devices,
    ) -> ntg_result;
    pub fn ntg_get_protocol(
        out: *mut ntg_protocol,
    ) -> ntg_result;
    pub fn ntg_enable_glib_loop(
        enable: bool,
    ) -> ntg_result;
    pub fn ntg_send_broadcast_timestamp(
        handle: *mut ntg_instance,
        chat_id: i64,
        timestamp: i64,
    ) -> ntg_result;
    pub fn ntg_send_broadcast_part(
        handle: *mut ntg_instance,
        chat_id: i64,
        segment_id: i64,
        part_id: i32,
        status: ntg_media_segment_part_status,
        quality_update: bool,
        data: *const u8, data_len: usize,
    ) -> ntg_result;
    pub fn ntg_send_signaling_data(
        handle: *mut ntg_instance,
        chat_id: i64,
        msg_key: *const u8, msg_key_len: usize,
    ) -> ntg_result;
    pub fn ntg_send_external_frame(
        handle: *mut ntg_instance,
        chat_id: i64,
        device: ntg_stream_device,
        data: *const u8, data_len: usize,
        frame_data: ntg_frame_data,
    ) -> ntg_result;
    pub fn ntg_update_audio_ssrc_mappings(
        handle: *mut ntg_instance,
        chat_id: i64,
        ssrc_groups: *const ntg_ssrc_mapping, ssrc_groups_len: usize,
    ) -> ntg_result;
    pub fn ntg_apply_blocks(
        handle: *mut ntg_instance,
        chat_id: i64,
        subchain: i32,
        next_offset: i32,
        blocks: *const ntg_bytes, blocks_len: usize,
        from_short_poll: bool,
    ) -> ntg_result;
    pub fn ntg_finish_subchain_request(
        handle: *mut ntg_instance,
        chat_id: i64,
        subchain: i32,
    ) -> ntg_result;
    pub fn ntg_calls(
        handle: *mut ntg_instance,
        out: *mut *mut ntg_call_info_entry, out_len: *mut usize,
    ) -> ntg_result;

    pub fn ntg_on_upgrade_callback(handle: *mut ntg_instance, callback: ntg_upgrade_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_stream_end_callback(handle: *mut ntg_instance, callback: ntg_stream_end_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_connection_change_callback(handle: *mut ntg_instance, callback: ntg_connection_change_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_frames_callback(handle: *mut ntg_instance, callback: ntg_frames_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_signaling_data_callback(handle: *mut ntg_instance, callback: ntg_signaling_data_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_remote_source_change_callback(handle: *mut ntg_instance, callback: ntg_remote_source_change_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_request_broadcast_part_callback(handle: *mut ntg_instance, callback: ntg_request_broadcast_part_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_request_broadcast_timestamp_callback(handle: *mut ntg_instance, callback: ntg_request_broadcast_timestamp_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_request_participants_callback(handle: *mut ntg_instance, callback: ntg_request_participants_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_outbound_block_callback(handle: *mut ntg_instance, callback: ntg_outbound_block_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_subchain_request_callback(handle: *mut ntg_instance, callback: ntg_subchain_request_callback_cb, user_data: *mut c_void) -> ntg_result;
    pub fn ntg_on_update_emojis_callback(handle: *mut ntg_instance, callback: ntg_update_emojis_callback_cb, user_data: *mut c_void) -> ntg_result;
}

#[cfg(glibc_resolv_compat)]
mod glibc_resolv_compat {
    use std::os::raw::{c_char, c_int, c_uchar, c_void};

    extern "C" {
        fn dn_expand(msg: *const c_uchar, eom: *const c_uchar, src: *const c_uchar, dst: *mut c_char, dstsiz: c_int) -> c_int;
        fn res_nquery(statp: *mut c_void, dname: *const c_char, class_: c_int, type_: c_int, answer: *mut c_uchar, anslen: c_int) -> c_int;
    }

    #[no_mangle]
    pub unsafe extern "C" fn __dn_expand(msg: *const c_uchar, eom: *const c_uchar, src: *const c_uchar, dst: *mut c_char, dstsiz: c_int) -> c_int {
        dn_expand(msg, eom, src, dst, dstsiz)
    }

    #[no_mangle]
    pub unsafe extern "C" fn __res_nquery(statp: *mut c_void, dname: *const c_char, class_: c_int, type_: c_int, answer: *mut c_uchar, anslen: c_int) -> c_int {
        res_nquery(statp, dname, class_, type_, answer, anslen)
    }
}
