//! xHCI device / slot / endpoint context structures (spec §6.2).
//!
//! Two layouts exist on real hardware:
//!   - **32-byte contexts** (legacy/AMD): CSZ=0 in HCCPARAMS1.
//!   - **64-byte contexts** (Intel, modern controllers, qemu-xhci): CSZ=1.
//!
//! Both layouts pack the same fields in the same low bytes — the extra
//! 32 bytes in the 64-byte variant are reserved padding. So we
//! *allocate* every context at 64-byte stride but the field accessors
//! are size-aware: writes go to the low bytes, reads pull from the low
//! bytes, padding is left zero. This means a single set of structures
//! works on both layouts AS LONG AS the stride between adjacent contexts
//! (which xHCI itself indexes by raw offset from the Input/Device
//! Context base) matches CSZ.
//!
//! We always **allocate** as 64-byte stride (matching qemu-xhci and
//! Intel hardware), and at runtime we *branch* on CSZ:
//! - CSZ=1 (Intel, qemu-xhci): the allocation matches the hardware
//!   expectation; we use it directly.
//! - CSZ=0 (AMD/legacy): we currently FAIL boot with a clear log line.
//!   Supporting CSZ=0 requires a parallel 32-byte allocation; deferred.
//!
//! The check lives in `xhci::probe_csz()` and is the documented branch
//! point for the metal-side test (qemu-xhci is always CSZ=1, so this
//! path is exercised; AMD hardware is the only place CSZ=0 shows up).
//!
//! # USB device descriptor (spec §9.6.1) — separate from xHCI contexts
//!
//! The xHCI's slot/endpoint contexts are CONTROLLER-SIDE state. The
//! USB device itself returns a `DeviceDescriptor` (and ConfigDescriptor,
//! InterfaceDescriptor, …) when we issue Setup transfers on EP0. Those
//! belong to the USB device model, not the controller. We park them in
//! this file because they form the union of "what we know about an
//! attached device."

#[repr(C, align(64))]
#[derive(Copy, Clone)]
pub struct SlotContext {
    /// dword 0: route string (19:0), speed (23:20), reserved, MTT (25),
    /// hub (26), context entries (31:27).
    pub dw0: u32,
    /// dword 1: max exit latency (15:0), root hub port number (23:16),
    /// number of ports (31:24).
    pub dw1: u32,
    /// dword 2: TT hub slot ID (7:0), TT port (15:8), TTT (17:16),
    /// reserved, interrupter (31:22).
    pub dw2: u32,
    /// dword 3: USB device address (7:0), reserved (26:8), slot state (31:27).
    pub dw3: u32,
    /// Reserved dwords for spec future expansion (16 bytes).
    pub reserved: [u32; 4],
    /// Padding to bring the structure up to 64 bytes (CSZ=1 layout).
    /// Hardware ignores this when CSZ=0.
    pub _csz1_pad: [u32; 8],
}

impl SlotContext {
    pub const fn zero() -> Self {
        Self { dw0: 0, dw1: 0, dw2: 0, dw3: 0, reserved: [0; 4], _csz1_pad: [0; 8] }
    }
    /// Build dw0 with context-entries=N (we always pass 1 for "EP0 only").
    #[inline]
    pub fn set_context_entries(&mut self, ce: u32) {
        self.dw0 = (self.dw0 & 0x07FF_FFFF) | ((ce & 0x1F) << 27);
    }
    /// Set the root hub port number (1-indexed in xHCI terms).
    #[inline]
    pub fn set_root_hub_port(&mut self, port: u8) {
        self.dw1 = (self.dw1 & 0xFF00_FFFF) | ((port as u32) << 16);
    }
    /// Set USB device speed (1..4: FS, LS, HS, SS, see spec §6.2.2 Table 6-9).
    #[inline]
    pub fn set_speed(&mut self, speed: u32) {
        self.dw0 = (self.dw0 & 0xFF0F_FFFF) | ((speed & 0xF) << 20);
    }
    /// Address assigned by Address Device.
    #[inline]
    pub fn usb_device_address(&self) -> u8 {
        (self.dw3 & 0xFF) as u8
    }
    /// Slot state (5 bits, top of dw3) — Enabled=1, Default=2, Addressed=3,
    /// Configured=4.
    #[inline]
    pub fn slot_state(&self) -> u32 {
        (self.dw3 >> 27) & 0x1F
    }
}

#[repr(C, align(64))]
#[derive(Copy, Clone)]
pub struct EndpointContext {
    /// dword 0: EP state (2:0), reserved, mult (9:8), max-pstreams (14:10),
    /// LSA (15), interval (23:16), max ESIT payload hi (31:24).
    pub dw0: u32,
    /// dword 1: reserved (1:0), CErr (3:2), EP type (5:3), reserved (6),
    /// HID (7), max burst size (15:8), max packet size (31:16).
    pub dw1: u32,
    /// dword 2: dequeue cycle state (0), reserved (3:1), TR dequeue ptr lo (31:4).
    pub dw2: u32,
    /// dword 3: TR dequeue ptr hi (31:0).
    pub dw3: u32,
    /// dword 4: average TRB length (15:0), max ESIT payload lo (31:16).
    pub dw4: u32,
    pub reserved: [u32; 3],
    /// 64-byte stride padding (CSZ=1 layout).
    pub _csz1_pad: [u32; 8],
}

impl EndpointContext {
    pub const fn zero() -> Self {
        Self {
            dw0: 0, dw1: 0, dw2: 0, dw3: 0, dw4: 0,
            reserved: [0; 3], _csz1_pad: [0; 8],
        }
    }
    /// Configure as a Control endpoint (EP0): EP type=4 (Control bidirectional),
    /// max packet size 8/16/32/64 depending on speed, average TRB len = 8.
    ///
    /// dw1 layout (matches Linux drivers/usb/host/xhci.h, xHCI rev 1.2 §6.2.3):
    ///   bit 0 = RsvdZ, bits 2:1 = CErr (Error Count, 2 bits),
    ///   bits 5:3 = EP Type (3 bits), bit 6 = RsvdZ, bit 7 = HID,
    ///   bits 15:8 = Max Burst Size, bits 31:16 = Max Packet Size.
    pub fn init_control_ep(
        &mut self,
        max_packet_size: u16,
        tr_dequeue_phys: u64,
        dequeue_cycle_state: bool,
    ) {
        const EP_TYPE_CONTROL: u32 = 4;
        const CERR: u32 = 3; // 3-strike error count (recommended)
        self.dw1 = (CERR << 1) | (EP_TYPE_CONTROL << 3) | ((max_packet_size as u32) << 16);
        let dcs = if dequeue_cycle_state { 1u32 } else { 0u32 };
        self.dw2 = dcs | ((tr_dequeue_phys & 0xFFFF_FFF0) as u32);
        self.dw3 = (tr_dequeue_phys >> 32) as u32;
        // Average TRB length: tiny default, recommended >= 8 for control.
        self.dw4 = 8;
    }

    /// Configure as an Interrupt-IN endpoint (HID keyboard). EP type=7.
    pub fn init_interrupt_in_ep(
        &mut self,
        max_packet_size: u16,
        interval_log2: u8,
        tr_dequeue_phys: u64,
        dequeue_cycle_state: bool,
    ) {
        const EP_TYPE_INTERRUPT_IN: u32 = 7;
        const CERR: u32 = 3;
        // Interval is exponent of microframes / frames per spec §6.2.3.6.
        self.dw0 = (interval_log2 as u32 & 0xFF) << 16;
        self.dw1 = (CERR << 1) | (EP_TYPE_INTERRUPT_IN << 3) | ((max_packet_size as u32) << 16);
        let dcs = if dequeue_cycle_state { 1u32 } else { 0u32 };
        self.dw2 = dcs | ((tr_dequeue_phys & 0xFFFF_FFF0) as u32);
        self.dw3 = (tr_dequeue_phys >> 32) as u32;
        self.dw4 = max_packet_size as u32;
    }
}

/// Input Context — used to issue Address Device and Configure Endpoint
/// commands. Layout: input control context, slot context, then 31
/// endpoint contexts (EP0 IN/OUT bidirectional + EP1..EP15 each direction).
/// Each at 64-byte stride.
#[repr(C, align(64))]
pub struct InputContext {
    /// Input Control Context — D=drop flags, A=add flags. Spec §6.2.5.
    pub icc_drop: u32,
    pub icc_add: u32,
    pub icc_reserved: [u32; 5],
    pub icc_config_value: u32,  // also Interface Number / Alt Setting on byte 1/2
    /// 64-byte stride padding for the input control context itself.
    pub icc_pad: [u32; 8],
    pub slot: SlotContext,
    /// EP0 is at index 0; EP1 OUT=1, EP1 IN=2, ..., EP15 OUT=29, EP15 IN=30.
    /// Spec §6.2.3 Table 6-16: dci = 2*ep_num + direction (1=IN), with
    /// EP0 special-cased as dci=1. So our array index = dci - 1.
    pub eps: [EndpointContext; 31],
}

impl InputContext {
    pub const fn zero() -> Self {
        Self {
            icc_drop: 0,
            icc_add: 0,
            icc_reserved: [0; 5],
            icc_config_value: 0,
            icc_pad: [0; 8],
            slot: SlotContext::zero(),
            eps: [EndpointContext::zero(); 31],
        }
    }
}

/// Device Context — written by hardware, read by software. Same layout
/// as the Input Context minus the Input Control Context.
#[repr(C, align(64))]
pub struct DeviceContext {
    pub slot: SlotContext,
    pub eps: [EndpointContext; 31],
}

impl DeviceContext {
    pub const fn zero() -> Self {
        Self {
            slot: SlotContext::zero(),
            eps: [EndpointContext::zero(); 31],
        }
    }
}

// ============================================================================
// USB-side descriptors (spec §9.6)
// ============================================================================

/// 18-byte USB Device Descriptor returned via GET_DESCRIPTOR on EP0.
#[repr(C, packed)]
#[derive(Copy, Clone, Default)]
pub struct DeviceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,    // 1 = DEVICE
    pub bcd_usb: u16,
    pub b_device_class: u8,
    pub b_device_subclass: u8,
    pub b_device_protocol: u8,
    pub b_max_packet_size0: u8,   // EP0 max packet size: 8/16/32/64
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_device: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    pub i_serial_number: u8,
    pub b_num_configurations: u8,
}

/// 9-byte Configuration Descriptor (spec §9.6.3).
#[repr(C, packed)]
#[derive(Copy, Clone, Default)]
pub struct ConfigDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,    // 2 = CONFIGURATION
    pub w_total_length: u16,      // total combined config+iface+ep+hid descriptors
    pub b_num_interfaces: u8,
    pub b_configuration_value: u8,
    pub i_configuration: u8,
    pub bm_attributes: u8,
    pub b_max_power: u8,
}

/// 9-byte Interface Descriptor (spec §9.6.5). For HID class = 0x03.
#[repr(C, packed)]
#[derive(Copy, Clone, Default)]
pub struct InterfaceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,    // 4 = INTERFACE
    pub b_interface_number: u8,
    pub b_alternate_setting: u8,
    pub b_num_endpoints: u8,
    pub b_interface_class: u8,    // 0x03 = HID
    pub b_interface_subclass: u8, // 0x01 = Boot
    pub b_interface_protocol: u8, // 0x01 = Keyboard, 0x02 = Mouse
    pub i_interface: u8,
}

/// 7-byte Endpoint Descriptor (spec §9.6.6).
#[repr(C, packed)]
#[derive(Copy, Clone, Default)]
pub struct EndpointDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,    // 5 = ENDPOINT
    pub b_endpoint_address: u8,   // bit 7 = direction (1=IN), bits 3:0 = ep number
    pub bm_attributes: u8,        // bits 1:0 = transfer type (3 = Interrupt)
    pub w_max_packet_size: u16,
    pub b_interval: u8,           // polling interval (frame units for LS/FS, microframe for HS)
}

/// USB Setup Stage Data (spec §9.3 Table 9-2) — the 8 bytes the host
/// sends on EP0 to start a control transfer.
#[repr(C, packed)]
#[derive(Copy, Clone, Default)]
pub struct SetupPacket {
    pub bm_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
}

/// Standard requests we issue.
pub mod request {
    pub const GET_STATUS: u8 = 0;
    pub const SET_ADDRESS: u8 = 5;
    pub const GET_DESCRIPTOR: u8 = 6;
    pub const SET_CONFIGURATION: u8 = 9;
    // HID class requests (bmRequestType=0x21 to set, 0xA1 to get on interface)
    pub const HID_SET_PROTOCOL: u8 = 0x0B;
    pub const HID_GET_REPORT: u8 = 0x01;
}

/// Descriptor types in the high byte of wValue for GET_DESCRIPTOR.
pub mod desc_type {
    pub const DEVICE: u8 = 1;
    pub const CONFIGURATION: u8 = 2;
    pub const STRING: u8 = 3;
    pub const INTERFACE: u8 = 4;
    pub const ENDPOINT: u8 = 5;
    pub const HID: u8 = 0x21;
    pub const HID_REPORT: u8 = 0x22;
}

/// USB class codes.
pub mod class {
    pub const HID: u8 = 0x03;
}
