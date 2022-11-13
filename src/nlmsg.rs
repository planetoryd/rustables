use std::{collections::BTreeMap, fmt::Debug, mem::size_of};

use crate::{
    parser::{
        pad_netlink_object, pad_netlink_object_with_variable_size, write_attribute, AttributeType,
        DecodeError,
    },
    sys::{
        nfgenmsg, nlattr, nlmsghdr, NFNETLINK_V0, NFNL_MSG_BATCH_BEGIN, NFNL_MSG_BATCH_END,
        NFNL_SUBSYS_NFTABLES,
    },
    MsgType, ProtocolFamily,
};

pub struct NfNetlinkWriter<'a> {
    buf: &'a mut Vec<u8>,
    headers: Option<(usize, usize)>,
}

impl<'a> NfNetlinkWriter<'a> {
    pub fn new(buf: &'a mut Vec<u8>) -> NfNetlinkWriter<'a> {
        NfNetlinkWriter { buf, headers: None }
    }

    pub fn add_data_zeroed<'b>(&'b mut self, size: usize) -> &'b mut [u8] {
        let padded_size = pad_netlink_object_with_variable_size(size);
        let start = self.buf.len();
        self.buf.resize(start + padded_size, 0);

        if let Some((msghdr_idx, _nfgenmsg_idx)) = self.headers {
            let mut hdr: &mut nlmsghdr = unsafe {
                std::mem::transmute(self.buf[msghdr_idx..].as_mut_ptr() as *mut nlmsghdr)
            };
            hdr.nlmsg_len += padded_size as u32;
        }

        &mut self.buf[start..start + size]
    }

    pub fn extract_buffer(self) -> &'a mut Vec<u8> {
        self.buf
    }

    // rewrite of `__nftnl_nlmsg_build_hdr`
    pub fn write_header(
        &mut self,
        msg_type: u16,
        family: ProtocolFamily,
        flags: u16,
        seq: u32,
        ressource_id: Option<u16>,
    ) {
        if self.headers.is_some() {
            error!("Calling write_header while still holding headers open!?");
        }

        let nlmsghdr_len = pad_netlink_object::<nlmsghdr>();
        let nfgenmsg_len = pad_netlink_object::<nfgenmsg>();

        let nlmsghdr_buf = self.add_data_zeroed(nlmsghdr_len);
        let mut hdr: &mut nlmsghdr =
            unsafe { std::mem::transmute(nlmsghdr_buf.as_mut_ptr() as *mut nlmsghdr) };
        hdr.nlmsg_len = (nlmsghdr_len + nfgenmsg_len) as u32;
        hdr.nlmsg_type = msg_type;
        // batch messages are not specific to the nftables subsystem
        if msg_type != NFNL_MSG_BATCH_BEGIN as u16 && msg_type != NFNL_MSG_BATCH_END as u16 {
            hdr.nlmsg_type |= (NFNL_SUBSYS_NFTABLES as u16) << 8;
        }
        hdr.nlmsg_flags = libc::NLM_F_REQUEST as u16 | flags;
        hdr.nlmsg_seq = seq;

        let nfgenmsg_buf = self.add_data_zeroed(nfgenmsg_len);
        let mut nfgenmsg: &mut nfgenmsg =
            unsafe { std::mem::transmute(nfgenmsg_buf.as_mut_ptr() as *mut nfgenmsg) };
        nfgenmsg.nfgen_family = family as u8;
        nfgenmsg.version = NFNETLINK_V0 as u8;
        nfgenmsg.res_id = ressource_id.unwrap_or(0);

        self.headers = Some((
            self.buf.len() - (nlmsghdr_len + nfgenmsg_len),
            self.buf.len() - nfgenmsg_len,
        ));
    }

    pub fn finalize_writing_object(&mut self) {
        self.headers = None;
    }
}

pub trait AttributeDecoder {
    fn decode_attribute(
        attrs: &NfNetlinkAttributes,
        attr_type: u16,
        buf: &[u8],
    ) -> Result<AttributeType, DecodeError>;
}

pub trait NfNetlinkDeserializable: Sized {
    fn deserialize(buf: &[u8]) -> Result<(Self, &[u8]), DecodeError>;
}

pub trait NfNetlinkObject: Sized + AttributeDecoder + NfNetlinkDeserializable {
    fn add_or_remove<'a>(&self, writer: &mut NfNetlinkWriter<'a>, msg_type: MsgType, seq: u32);
}

pub type NetlinkType = u16;

pub trait NfNetlinkAttribute: Debug + Sized {
    // is it a nested argument that must be marked with a NLA_F_NESTED flag?
    fn is_nested(&self) -> bool {
        false
    }

    fn get_size(&self) -> usize {
        size_of::<Self>()
    }

    // example body: std::ptr::copy_nonoverlapping(self as *const Self as *const u8, addr, self.get_size());
    unsafe fn write_payload(&self, addr: *mut u8);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NfNetlinkAttributes {
    pub attributes: BTreeMap<NetlinkType, AttributeType>,
}

impl NfNetlinkAttributes {
    pub fn new() -> Self {
        NfNetlinkAttributes {
            attributes: BTreeMap::new(),
        }
    }

    pub fn set_attr(&mut self, ty: NetlinkType, obj: AttributeType) {
        self.attributes.insert(ty, obj);
    }

    pub fn get_attr(&self, ty: NetlinkType) -> Option<&AttributeType> {
        self.attributes.get(&ty)
    }

    pub fn serialize<'a>(&self, writer: &mut NfNetlinkWriter<'a>) {
        let buf = writer.add_data_zeroed(self.get_size());
        unsafe {
            self.write_payload(buf.as_mut_ptr());
        }
    }
}

impl NfNetlinkAttribute for NfNetlinkAttributes {
    fn get_size(&self) -> usize {
        let mut size = 0;

        for (_type, attr) in self.attributes.iter() {
            // Attribute header + attribute value
            size += pad_netlink_object::<nlattr>()
                + pad_netlink_object_with_variable_size(attr.get_size());
        }

        size
    }

    unsafe fn write_payload(&self, mut addr: *mut u8) {
        for (ty, attr) in self.attributes.iter() {
            debug!("writing attribute {} - {:?}", ty, attr);
            write_attribute(*ty, attr, addr);
            let size = pad_netlink_object::<nlattr>()
                + pad_netlink_object_with_variable_size(attr.get_size());
            addr = addr.offset(size as isize);
        }
    }
}
