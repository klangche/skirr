//! Data model for USB diagnostics - normalized across all platforms

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// USB device class codes (from USB spec)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum UsbClass {
    Unspecified = 0x00,
    Audio = 0x01,
    Communications = 0x02,
    HID = 0x03,
    Physical = 0x05,
    Image = 0x06,
    Printer = 0x07,
    MassStorage = 0x08,
    Hub = 0x09,
    CDCData = 0x0A,
    SmartCard = 0x0B,
    ContentSecurity = 0x0D,
    Video = 0x0E,
    PersonalHealthcare = 0x0F,
    AudioVideo = 0x10,
    Billboard = 0x11,
    USBTypeCBridge = 0x12,
    DiagnosticDevice = 0xDC,
    WirelessController = 0xE0,
    Miscellaneous = 0xEF,
    ApplicationSpecific = 0xFE,
    VendorSpecific = 0xFF,
}

impl UsbClass {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0x00 => UsbClass::Unspecified,
            0x01 => UsbClass::Audio,
            0x02 => UsbClass::Communications,
            0x03 => UsbClass::HID,
            0x05 => UsbClass::Physical,
            0x06 => UsbClass::Image,
            0x07 => UsbClass::Printer,
            0x08 => UsbClass::MassStorage,
            0x09 => UsbClass::Hub,
            0x0A => UsbClass::CDCData,
            0x0B => UsbClass::SmartCard,
            0x0D => UsbClass::ContentSecurity,
            0x0E => UsbClass::Video,
            0x0F => UsbClass::PersonalHealthcare,
            0x10 => UsbClass::AudioVideo,
            0x11 => UsbClass::Billboard,
            0x12 => UsbClass::USBTypeCBridge,
            0xDC => UsbClass::DiagnosticDevice,
            0xE0 => UsbClass::WirelessController,
            0xEF => UsbClass::Miscellaneous,
            0xFE => UsbClass::ApplicationSpecific,
            0xFF => UsbClass::VendorSpecific,
            _ => UsbClass::Unspecified,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            UsbClass::Unspecified => "Unspecified",
            UsbClass::Audio => "Audio",
            UsbClass::Communications => "Communications",
            UsbClass::HID => "HID",
            UsbClass::Physical => "Physical",
            UsbClass::Image => "Image",
            UsbClass::Printer => "Printer",
            UsbClass::MassStorage => "Mass Storage",
            UsbClass::Hub => "Hub",
            UsbClass::CDCData => "CDC Data",
            UsbClass::SmartCard => "Smart Card",
            UsbClass::ContentSecurity => "Content Security",
            UsbClass::Video => "Video",
            UsbClass::PersonalHealthcare => "Personal Healthcare",
            UsbClass::AudioVideo => "Audio/Video",
            UsbClass::Billboard => "Billboard",
            UsbClass::USBTypeCBridge => "USB Type-C Bridge",
            UsbClass::DiagnosticDevice => "Diagnostic Device",
            UsbClass::WirelessController => "Wireless Controller",
            UsbClass::Miscellaneous => "Miscellaneous",
            UsbClass::ApplicationSpecific => "Application Specific",
            UsbClass::VendorSpecific => "Vendor Specific",
        }
    }
}

/// USB speed enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum UsbSpeed {
    Unknown = 0,
    LowSpeed = 1,
    FullSpeed = 2,
    HighSpeed = 3,
    SuperSpeed = 4,
    SuperSpeedPlus10 = 5,
    SuperSpeedPlus20 = 6,
    USB4Gen2x2 = 7,
    USB4Gen3x2 = 8,
    USB4Gen4x2 = 9,
}

impl UsbSpeed {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => UsbSpeed::Unknown,
            1 => UsbSpeed::LowSpeed,
            2 => UsbSpeed::FullSpeed,
            3 => UsbSpeed::HighSpeed,
            4 => UsbSpeed::SuperSpeed,
            5 => UsbSpeed::SuperSpeedPlus10,
            6 => UsbSpeed::SuperSpeedPlus20,
            7 => UsbSpeed::USB4Gen2x2,
            8 => UsbSpeed::USB4Gen3x2,
            9 => UsbSpeed::USB4Gen4x2,
            _ => UsbSpeed::Unknown,
        }
    }

    pub fn mbps(&self) -> u64 {
        match self {
            UsbSpeed::Unknown => 0,
            UsbSpeed::LowSpeed => 1,
            UsbSpeed::FullSpeed => 12,
            UsbSpeed::HighSpeed => 480,
            UsbSpeed::SuperSpeed => 5000,
            UsbSpeed::SuperSpeedPlus10 => 10000,
            UsbSpeed::SuperSpeedPlus20 => 20000,
            UsbSpeed::USB4Gen2x2 => 20000,
            UsbSpeed::USB4Gen3x2 => 40000,
            UsbSpeed::USB4Gen4x2 => 80000,
        }
    }

    pub fn gbps(&self) -> f64 {
        self.mbps() as f64 / 1000.0
    }

    pub fn marketing_name(&self) -> &'static str {
        match self {
            UsbSpeed::Unknown => "Unknown",
            UsbSpeed::LowSpeed => "USB 1.0 Low Speed (1.5 Mbps)",
            UsbSpeed::FullSpeed => "USB 1.1 Full Speed (12 Mbps)",
            UsbSpeed::HighSpeed => "USB 2.0 High Speed (480 Mbps)",
            UsbSpeed::SuperSpeed => "USB 3.0/3.1 Gen 1 (5 Gbps)",
            UsbSpeed::SuperSpeedPlus10 => "USB 3.1 Gen 2 (10 Gbps)",
            UsbSpeed::SuperSpeedPlus20 => "USB 3.2 Gen 2x2 (20 Gbps)",
            UsbSpeed::USB4Gen2x2 => "USB4 Gen 2x2 (20 Gbps)",
            UsbSpeed::USB4Gen3x2 => "USB4 Gen 3x2 (40 Gbps)",
            UsbSpeed::USB4Gen4x2 => "USB4 v2.0 Gen 4x2 (80 Gbps)",
        }
    }

    pub fn is_usb3_or_higher(&self) -> bool {
        matches!(
            self,
            UsbSpeed::SuperSpeed
                | UsbSpeed::SuperSpeedPlus10
                | UsbSpeed::SuperSpeedPlus20
                | UsbSpeed::USB4Gen2x2
                | UsbSpeed::USB4Gen3x2
                | UsbSpeed::USB4Gen4x2
        )
    }
}

/// USB device descriptor information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    pub usb_version: String,
    pub device_class: UsbClass,
    pub device_subclass: u8,
    pub device_protocol: u8,
    pub max_packet_size_0: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub device_version: String,
    pub manufacturer_index: Option<u8>,
    pub product_index: Option<u8>,
    pub serial_number_index: Option<u8>,
    pub num_configurations: u8,
}

/// USB configuration descriptor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigurationDescriptor {
    pub configuration_value: u8,
    pub configuration_index: Option<u8>,
    pub attributes: u8,
    pub max_power_ma: u16,
    pub interfaces: Vec<InterfaceDescriptor>,
}

/// USB interface descriptor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceDescriptor {
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub interface_class: UsbClass,
    pub interface_subclass: u8,
    pub interface_protocol: u8,
    pub interface_index: Option<u8>,
    pub endpoints: Vec<EndpointDescriptor>,
}

/// USB endpoint descriptor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointDescriptor {
    pub endpoint_address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

/// Complete USB device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbDevice {
    pub id: Uuid,
    pub platform_id: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub device_class: UsbClass,
    pub device_subclass: u8,
    pub device_protocol: u8,
    pub device_descriptor: Option<DeviceDescriptor>,
    pub configurations: Vec<ConfigurationDescriptor>,
    pub max_supported_speed: UsbSpeed,
    pub current_link_speed: UsbSpeed,
    pub is_hub: bool,
    pub hub_info: Option<HubInfo>,
    pub parent_id: Option<Uuid>,
    pub children_ids: Vec<Uuid>,
    pub port_number: Option<u8>,
    pub topology_depth: u8,
    pub hop_count: u8,
    pub tier: u8,
    pub host_controller_id: Option<Uuid>,
    pub root_hub_id: Option<Uuid>,
    pub connection_status: ConnectionStatus,
    pub usb_c_info: Option<UsbCInfo>,
    pub thunderbolt_info: Option<ThunderboltInfo>,
    pub usb4_info: Option<Usb4Info>,
    pub power_info: Option<PowerInfo>,
    pub last_seen: DateTime<Utc>,
    pub first_seen: DateTime<Utc>,
    pub is_new: bool,
    pub properties: HashMap<String, String>,
}

impl UsbDevice {
    pub fn new(vendor_id: u16, product_id: u16) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            platform_id: String::new(),
            vendor_id,
            product_id,
            manufacturer: None,
            product: None,
            serial_number: None,
            device_class: UsbClass::Unspecified,
            device_subclass: 0,
            device_protocol: 0,
            device_descriptor: None,
            configurations: Vec::new(),
            max_supported_speed: UsbSpeed::Unknown,
            current_link_speed: UsbSpeed::Unknown,
            is_hub: false,
            hub_info: None,
            parent_id: None,
            children_ids: Vec::new(),
            port_number: None,
            topology_depth: 0,
            hop_count: 0,
            tier: 0,
            host_controller_id: None,
            root_hub_id: None,
            connection_status: ConnectionStatus::Connected,
            usb_c_info: None,
            thunderbolt_info: None,
            usb4_info: None,
            power_info: None,
            last_seen: now,
            first_seen: now,
            is_new: true,
            properties: HashMap::new(),
        }
    }

    pub fn speed_bottleneck(&self) -> Option<SpeedBottleneck> {
        if self.max_supported_speed > self.current_link_speed
            && self.current_link_speed != UsbSpeed::Unknown
        {
            Some(SpeedBottleneck {
                device_id: self.id,
                max_speed: self.max_supported_speed,
                current_speed: self.current_link_speed,
                severity: BottleneckSeverity::from_speeds(
                    self.max_supported_speed,
                    self.current_link_speed,
                ),
            })
        } else {
            None
        }
    }
}

/// Speed bottleneck detection result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedBottleneck {
    pub device_id: Uuid,
    pub max_speed: UsbSpeed,
    pub current_speed: UsbSpeed,
    pub severity: BottleneckSeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BottleneckSeverity {
    Minor,
    Major,
    Critical,
}

impl BottleneckSeverity {
    pub fn from_speeds(max: UsbSpeed, current: UsbSpeed) -> Self {
        let max_mbps = max.mbps();
        let current_mbps = current.mbps();

        if current_mbps == 0 {
            return BottleneckSeverity::Critical;
        }

        let ratio = max_mbps as f64 / current_mbps as f64;

        if ratio >= 10.0 {
            BottleneckSeverity::Critical
        } else if ratio >= 2.0 {
            BottleneckSeverity::Major
        } else {
            BottleneckSeverity::Minor
        }
    }
}

/// Connection status of a device
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    Connected,
    Disconnected,
    ReEnumerated,
    Error,
    Unknown,
}

/// Hub information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubInfo {
    pub port_count: u8,
    pub is_powered: bool,
    pub power_source: HubPowerSource,
    pub supports_mtt: bool,
    pub tt_count: u8,
    pub tt_type: HubTTType,
    pub hub_speed: UsbSpeed,
    pub ports: Vec<HubPort>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HubPowerSource {
    Unknown,
    BusPowered,
    SelfPowered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HubTTType {
    Unknown,
    SingleTT,
    MultiTT,
}

/// Hub port information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubPort {
    pub port_number: u8,
    pub connected_device_id: Option<Uuid>,
    pub port_speed: UsbSpeed,
    pub port_power_state: PortPowerState,
    pub port_connection_status: PortConnectionStatus,
    pub port_status: u16,
    pub port_change: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortPowerState {
    Unknown,
    Off,
    On,
    Suspend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortConnectionStatus {
    Unknown,
    NoDevice,
    DevicePresent,
    DeviceEnabled,
    DeviceError,
}

/// USB-C specific information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbCInfo {
    pub port_type: UsbCPortType,
    pub current_mode: UsbCCurrentMode,
    pub pd_supported: bool,
    pub pd_revision: Option<String>,
    pub alt_modes: Vec<AltModeInfo>,
    pub cable_info: Option<CableInfo>,
    pub connector_orientation: Option<ConnectorOrientation>,
    pub port_index: Option<u8>,
    pub partner_info: Option<PartnerInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsbCPortType {
    Unknown,
    DFP,
    UFP,
    DRP,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsbCCurrentMode {
    Unknown,
    DefaultUsb,
    TypeCCurrent1_5A,
    TypeCCurrent3_0A,
    UsbPd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AltModeInfo {
    pub svid: u16,
    pub mode: u8,
    pub description: String,
    pub active: bool,
    pub vdo: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CableInfo {
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub usb_version: Option<UsbSpeed>,
    pub max_power_watts: Option<u16>,
    pub max_voltage_mv: Option<u16>,
    pub max_current_ma: Option<u16>,
    pub cable_type: CableType,
    pub length_meters: Option<f32>,
    pub has_e_marker: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CableType {
    Unknown,
    Passive,
    Active,
    Optical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectorOrientation {
    Unknown,
    Normal,
    Flipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartnerInfo {
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub pd_revision: Option<String>,
    pub power_role: PowerRole,
    pub data_role: DataRole,
    pub supported_voltages: Vec<u16>,
    pub supported_currents: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerRole {
    Unknown,
    Source,
    Sink,
    DRP,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRole {
    Unknown,
    DFP,
    UFP,
    DRP,
}

/// Thunderbolt information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThunderboltInfo {
    pub is_thunderbolt: bool,
    pub generation: Option<ThunderboltGeneration>,
    pub speed: Option<UsbSpeed>,
    pub topology: Option<ThunderboltTopology>,
    pub security_level: Option<ThunderboltSecurityLevel>,
    pub nvm_version: Option<String>,
    pub device_uuid: Option<Uuid>,
    pub route_string: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThunderboltGeneration {
    Unknown,
    Thunderbolt1,
    Thunderbolt2,
    Thunderbolt3,
    Thunderbolt4,
    Thunderbolt5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThunderboltSecurityLevel {
    Unknown,
    NoSecurity,
    UserAuthorization,
    SecureConnect,
    DPOnly,
    UsbOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThunderboltTopology {
    pub domain: u32,
    pub route: u64,
    pub depth: u8,
    pub parent_route: Option<u64>,
}

/// USB4 information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usb4Info {
    pub is_usb4: bool,
    pub generation: Option<Usb4Generation>,
    pub speed: Option<UsbSpeed>,
    pub router_info: Option<Usb4RouterInfo>,
    pub capabilities: Usb4Capabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Usb4Generation {
    Unknown,
    Gen2x2,
    Gen3x2,
    Gen4x2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usb4RouterInfo {
    pub router_id: u16,
    pub adapter_count: u8,
    pub hop_id: u8,
    pub depth: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usb4Capabilities {
    pub pcie_tunneling: bool,
    pub dp_tunneling: bool,
    pub usb3_tunneling: bool,
    pub host_to_host: bool,
}

/// Power delivery information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerInfo {
    pub source_capabilities: Vec<PowerDataObject>,
    pub sink_capabilities: Vec<PowerDataObject>,
    pub negotiated_pdo: Option<PowerDataObject>,
    pub contract_voltage_mv: Option<u16>,
    pub contract_current_ma: Option<u16>,
    pub contract_power_mw: Option<u32>,
    pub pps_supported: bool,
    pub pps_voltage_range: Option<(u16, u16)>,
    pub pps_max_current_ma: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerDataObject {
    pub pdo_type: PdoType,
    pub voltage_mv: u16,
    pub current_ma: u16,
    pub max_power_mw: u32,
    pub flags: PdoFlags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PdoType {
    FixedSupply,
    VariableSupply,
    BatterySupply,
    AugmentedPower,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdoFlags {
    pub dual_role_power: bool,
    pub usb_suspend: bool,
    pub unconstrained_power: bool,
    pub higher_capability: bool,
    pub dual_role_data: bool,
}

/// Display/Monitor information with EDID
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: Uuid,
    pub platform_id: String,
    pub manufacturer_id: Option<String>,
    pub product_code: Option<u16>,
    pub serial_number: Option<u32>,
    pub manufacture_week: Option<u8>,
    pub manufacture_year: Option<u16>,
    pub edid_version: Option<String>,
    pub name: Option<String>,
    pub serial_number_str: Option<String>,
    pub max_horizontal_size_cm: Option<u16>,
    pub max_vertical_size_cm: Option<u16>,
    pub supported_resolutions: Vec<DisplayResolution>,
    pub preferred_resolution: Option<DisplayResolution>,
    pub current_resolution: Option<DisplayResolution>,
    pub refresh_rates: Vec<u16>,
    pub current_refresh_rate: Option<u16>,
    pub color_depth: Option<u8>,
    pub hdr_supported: bool,
    pub hdr_metadata: Option<HdrMetadata>,
    pub display_type: DisplayType,
    pub connection_type: Option<DisplayConnectionType>,
    pub gpu_id: Option<Uuid>,
    pub usb_path: Option<Vec<Uuid>>,
    pub is_primary: bool,
    pub is_internal: bool,
    pub is_enabled: bool,
    pub position: Option<DisplayPosition>,
    pub scale_factor: Option<f32>,
    pub edid_raw: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayResolution {
    pub width: u16,
    pub height: u16,
    pub is_interlaced: bool,
    pub aspect_ratio: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HdrMetadata {
    pub electro_optical_transfer_function: Vec<String>,
    pub static_metadata_type1: bool,
    pub desired_content_max_luminance: Option<u16>,
    pub desired_frame_max_luminance: Option<u16>,
    pub desired_min_luminance: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayType {
    Unknown,
    Internal,
    External,
    Virtual,
    Projector,
    TV,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayConnectionType {
    Unknown,
    HDMI,
    DisplayPort,
    DVI,
    VGA,
    /// USB-C connection
    #[serde(rename = "USB_C")]
    UsbC,
    Thunderbolt,
    Wireless,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayPosition {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Host controller information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostController {
    pub id: Uuid,
    pub platform_id: String,
    pub name: String,
    pub vendor_id: Option<u16>,
    pub device_id: Option<u16>,
    pub revision: Option<u8>,
    pub usb_version: UsbSpeed,
    pub root_hub_ids: Vec<Uuid>,
    pub port_count: u8,
    pub is_xhci: bool,
    pub pci_address: Option<String>,
    pub driver_version: Option<String>,
    pub capabilities: HostControllerCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostControllerCapabilities {
    pub supports_usb2: bool,
    pub supports_usb3: bool,
    pub supports_usb4: bool,
    pub supports_thunderbolt: bool,
    pub max_ports: u8,
    pub supports_lpm: bool,
    pub supports_l1pm: bool,
}

/// Root hub information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootHub {
    pub id: Uuid,
    pub platform_id: String,
    pub host_controller_id: Uuid,
    pub port_count: u8,
    pub hub_speed: UsbSpeed,
    pub is_integrated: bool,
    pub children_ids: Vec<Uuid>,
}

/// Diagnostic event for live monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticEvent {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub event_type: EventType,
    pub device_id: Option<Uuid>,
    pub hub_id: Option<Uuid>,
    pub port_number: Option<u8>,
    pub details: String,
    pub severity: EventSeverity,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    DeviceConnected,
    DeviceDisconnected,
    DeviceReEnumerated,
    HubConnected,
    HubDisconnected,
    TopologyChanged,
    SpeedChanged,
    PowerChanged,
    DisplayConnected,
    DisplayDisconnected,
    DisplayModeChanged,
    ThunderboltEvent,
    Usb4Event,
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Complete system topology snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemTopology {
    pub timestamp: DateTime<Utc>,
    pub host_controllers: Vec<HostController>,
    pub root_hubs: Vec<RootHub>,
    pub devices: Vec<UsbDevice>,
    pub hubs: Vec<UsbDevice>,
    pub displays: Vec<DisplayInfo>,
    pub events: Vec<DiagnosticEvent>,
    pub platform_info: PlatformInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub os: String,
    pub os_version: String,
    pub kernel_version: Option<String>,
    pub architecture: String,
    pub hostname: Option<String>,
    pub username: Option<String>,
    pub is_admin: bool,
    pub is_virtual_machine: bool,
    pub boot_time: Option<DateTime<Utc>>,
}

/// Complete diagnostic result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticResult {
    pub timestamp: DateTime<Utc>,
    pub profile_version: String,
    pub overall_verdict: Verdict,
    pub facts: Vec<Fact>,
    pub rules_applied: Vec<RuleEvaluation>,
    pub bottlenecks: Vec<SpeedBottleneck>,
    pub topology_issues: Vec<TopologyIssue>,
    pub display_issues: Vec<DisplayIssue>,
    pub usb_c_issues: Vec<UsbCIssue>,
    pub power_issues: Vec<PowerIssue>,
    pub event_summary: EventSummary,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Pass,
    Warning,
    Fail,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub id: String,
    pub category: FactCategory,
    pub description: String,
    pub value: serde_json::Value,
    pub source: String,
    pub confidence: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FactCategory {
    Topology,
    Speed,
    Power,
    Display,
    UsbC,
    Thunderbolt,
    Usb4,
    Device,
    Hub,
    HostController,
    Event,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleEvaluation {
    pub rule_id: String,
    pub rule_description: String,
    pub rule_category: FactCategory,
    pub expected: serde_json::Value,
    pub actual: serde_json::Value,
    pub verdict: Verdict,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyIssue {
    pub device_id: Uuid,
    pub issue_type: TopologyIssueType,
    pub description: String,
    pub severity: Verdict,
    pub current_value: u8,
    pub max_allowed: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TopologyIssueType {
    TooManyHubs,
    TooManyHops,
    TooManyTiers,
    ExcessiveDepth,
    HubChainTooLong,
    MissingRootHub,
    OrphanedDevice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayIssue {
    pub display_id: Uuid,
    pub issue_type: DisplayIssueType,
    pub description: String,
    pub severity: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayIssueType {
    NoEdid,
    ResolutionMismatch,
    RefreshRateMismatch,
    HdrNotSupported,
    ConnectionTypeUnknown,
    GpuNotIdentified,
    UsbPathBroken,
    SuboptimalResolution,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbCIssue {
    pub device_id: Uuid,
    pub issue_type: UsbCIssueType,
    pub description: String,
    pub severity: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsbCIssueType {
    AltModeNotActive,
    PdNotNegotiated,
    CableLimitation,
    PortTypeMismatch,
    PowerRoleMismatch,
    DataRoleMismatch,
    UnknownCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerIssue {
    pub device_id: Uuid,
    pub issue_type: PowerIssueType,
    pub description: String,
    pub severity: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerIssueType {
    InsufficientPower,
    VoltageMismatch,
    CurrentLimited,
    PdNegotiationFailed,
    NoPowerData,
    PpsNotAvailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub total_events: usize,
    pub connects: usize,
    pub disconnects: usize,
    pub re_enumerations: usize,
    pub speed_changes: usize,
    pub power_changes: usize,
    pub display_changes: usize,
    pub errors: usize,
    pub warnings: usize,
    pub monitoring_duration_secs: u64,
    pub stability_score: u8,
}
