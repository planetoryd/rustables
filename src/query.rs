use std::os::unix::prelude::RawFd;

use libc::c_int;
use netlink_sys::{AsyncSocket, AsyncSocketExt, Socket};
use nix::sys::socket::{self, AddressFamily, MsgFlags, SockFlag, SockProtocol, SockType};

use crate::{
    error::QueryError,
    nlmsg::{
        nft_nlmsg_maxsize, pad_netlink_object_with_variable_size, NfNetlinkAttribute,
        NfNetlinkObject, NfNetlinkWriter,
    },
    parser::{parse_nlmsg, NlMsg},
    sys::{NLM_F_DUMP, NLM_F_MULTI},
    ProtocolFamily,
};

pub(crate) fn recv_and_process<'a, T>(
    sock: &mut Socket,
    max_seq: Option<u32>,
    cb: Option<&dyn Fn(&[u8], &mut T) -> Result<(), QueryError>>,
    working_data: &'a mut T,
) -> anyhow::Result<()> {
    let mut msg_buffer = vec![0; 2 * nft_nlmsg_maxsize() as usize];
    let mut buf_start = 0;
    let mut end_pos = 0;
    debug!("recv_and_process");
    loop {
        debug!("recv_and_process nb_recv");

        let nb_recv = sock.recv(&mut msg_buffer[buf_start..], MsgFlags::empty().bits())?;
        if nb_recv <= 0 {
            return Ok(());
        }
        end_pos += nb_recv;
        loop {
            let buf = &msg_buffer[buf_start..end_pos];
            // exit the loop and try to receive further messages when we consumed all the buffer
            if buf.len() == 0 {
                break;
            }

            debug!("Calling parse_nlmsg");
            let (nlmsghdr, msg) = parse_nlmsg(&buf)?;
            debug!("Got a valid netlink message: {:?} {:?}", nlmsghdr, msg);

            match msg {
                NlMsg::Done => {
                    debug!("NlMsg::Done");
                    return Ok(());
                }
                NlMsg::Error(e) => {
                    if e.error != 0 {
                        // Ex. Do you have enough perms
                        return Err(QueryError::NetlinkError(e).into());
                    }
                }
                NlMsg::Noop => {}
                NlMsg::NfGenMsg(_genmsg, _data) => {
                    if let Some(cb) = cb {
                        cb(&buf[0..nlmsghdr.nlmsg_len as usize], working_data)?;
                    }
                }
            }

            // we cannot know when a sequence of messages will end if the messages do not end
            // with an NlMsg::Done marker if a maximum sequence number wasn't specified
            if max_seq.is_none() && nlmsghdr.nlmsg_flags & NLM_F_MULTI as u16 == 0 {
                return Err(QueryError::UndecidableMessageTermination.into());
            }

            // retrieve the next message
            if let Some(max_seq) = max_seq {
                if nlmsghdr.nlmsg_seq >= max_seq {
                    return Ok(());
                }
            }

            // netlink messages are 4bytes aligned
            let aligned_length = pad_netlink_object_with_variable_size(nlmsghdr.nlmsg_len as usize);
            buf_start += aligned_length;
        }
        // Ensure that we always have nft_nlmsg_maxsize() free space available in the buffer.
        // We achieve this by relocating the buffer content at the beginning of the buffer
        if end_pos >= nft_nlmsg_maxsize() as usize {
            if buf_start < end_pos {
                msg_buffer.copy_within(buf_start..end_pos, 0);
            }
            end_pos = end_pos - buf_start;
            buf_start = 0;
        }
    }
}

pub(crate) async fn recv_and_process_async<'a, T, S: AsyncSocket>(
    sock: &mut S,
    max_seq: Option<u32>,
    cb: Option<&(dyn (Fn(&[u8], &mut T) -> Result<(), QueryError>) + Send + Sync)>,
    working_data: &'a mut T,
) -> anyhow::Result<()> {
    let mut msg_buffer = vec![0; 2 * nft_nlmsg_maxsize() as usize];
    let mut buf_start = 0;
    let mut end_pos = 0;
    debug!("recv_and_process");
    loop {
        debug!("recv_and_process nb_recv");

        let nb_recv = sock.recv(&mut msg_buffer[buf_start..]).await?;
        if nb_recv <= 0 {
            return Ok(());
        }
        end_pos += nb_recv;
        loop {
            let buf = &msg_buffer[buf_start..end_pos];
            // exit the loop and try to receive further messages when we consumed all the buffer
            if buf.len() == 0 {
                break;
            }

            debug!("Calling parse_nlmsg");
            let (nlmsghdr, msg) = parse_nlmsg(&buf)?;
            debug!("Got a valid netlink message: {:?} {:?}", nlmsghdr, msg);

            match msg {
                NlMsg::Done => {
                    debug!("NlMsg::Done");
                    return Ok(());
                }
                NlMsg::Error(e) => {
                    if e.error != 0 {
                        // Ex. Do you have enough perms
                        return Err(QueryError::NetlinkError(e).into());
                    }
                }
                NlMsg::Noop => {}
                NlMsg::NfGenMsg(_genmsg, _data) => {
                    if let Some(cb) = cb {
                        cb(&buf[0..nlmsghdr.nlmsg_len as usize], working_data)?;
                    }
                }
            }

            // we cannot know when a sequence of messages will end if the messages do not end
            // with an NlMsg::Done marker if a maximum sequence number wasn't specified
            if max_seq.is_none() && nlmsghdr.nlmsg_flags & NLM_F_MULTI as u16 == 0 {
                return Err(QueryError::UndecidableMessageTermination.into());
            }

            // retrieve the next message
            if let Some(max_seq) = max_seq {
                if nlmsghdr.nlmsg_seq >= max_seq {
                    return Ok(());
                }
            }

            // netlink messages are 4bytes aligned
            let aligned_length = pad_netlink_object_with_variable_size(nlmsghdr.nlmsg_len as usize);
            buf_start += aligned_length;
        }
        // Ensure that we always have nft_nlmsg_maxsize() free space available in the buffer.
        // We achieve this by relocating the buffer content at the beginning of the buffer
        if end_pos >= nft_nlmsg_maxsize() as usize {
            if buf_start < end_pos {
                msg_buffer.copy_within(buf_start..end_pos, 0);
            }
            end_pos = end_pos - buf_start;
            buf_start = 0;
        }
    }
}

pub(crate) fn socket_close_wrapper<E>(
    sock: RawFd,
    cb: impl FnOnce(RawFd) -> Result<(), E>,
) -> Result<(), QueryError>
where
    QueryError: From<E>,
{
    let ret = cb(sock);

    // we don't need to shutdown the socket (in fact, Linux doesn't support that operation;
    // and return EOPNOTSUPP if we try)
    nix::unistd::close(sock).map_err(QueryError::CloseFailed)?;

    Ok(ret?)
}

/// Returns a buffer containing a netlink message which requests a list of all the netfilter
/// matching objects (e.g. tables, chains, rules, ...).
/// Supply the type of objects to retrieve (e.g. libc::NFT_MSG_GETTABLE), and a search filter.
pub fn get_list_of_objects<T: NfNetlinkAttribute>(
    msg_type: u16,
    seq: u32,
    filter: Option<&T>,
    family: ProtocolFamily,
) -> Result<Vec<u8>, QueryError> {
    let mut buffer = Vec::new();
    let mut writer = NfNetlinkWriter::new(&mut buffer);
    writer.write_header(msg_type, family, NLM_F_DUMP as u16, seq, None);
    if let Some(filter) = filter {
        let buf = writer.add_data_zeroed(filter.get_size());
        filter.write_payload(buf);
    }
    writer.finalize_writing_object();
    Ok(buffer)
}

/// Lists objects of a certain type (e.g. libc::NFT_MSG_GETTABLE) with the help of a helper
/// function called by mnl::cb_run2.
/// The callback expects a tuple of additional data (supplied as an argument to this function)
/// and of the output vector, to which it should append the parsed object it received.
pub fn list_objects_with_data<'a, Object, Accumulator>(
    data_type: u16,
    cb: &dyn Fn(Object, &mut Accumulator) -> Result<(), QueryError>,
    filter: Option<&Object>,
    working_data: &'a mut Accumulator,
    sock: &mut Socket,
) -> anyhow::Result<()>
where
    Object: NfNetlinkObject + NfNetlinkAttribute,
{
    debug!("Listing objects of kind {}", data_type);

    let seq = 0;
    let chains_buf = get_list_of_objects(
        data_type,
        seq,
        filter,
        if filter.is_some() {
            filter.unwrap().get_family()
        } else {
            ProtocolFamily::Unspec
            // this probably won't get you anything. usually programming error
        },
    )?;
    sock.send(&chains_buf, MsgFlags::empty().bits())?;

    recv_and_process(
        sock,
        None,
        Some(&|buf: &[u8], working_data: &mut Accumulator| {
            debug!("Calling Object::deserialize()");
            cb(Object::deserialize(buf)?.0, working_data)
        }),
        working_data,
    )
}

pub async fn list_objects_with_data_async<'a, Object, Accumulator, S: AsyncSocket>(
    data_type: u16,
    cb: &(dyn (Fn(Object, &mut Accumulator) -> Result<(), QueryError>) + Send + Sync),
    filter: Option<&Object>,
    working_data: &'a mut Accumulator,
    sock: &mut S,
) -> anyhow::Result<()>
where
    Object: NfNetlinkObject + NfNetlinkAttribute,
{
    debug!("Listing objects of kind {}", data_type);

    let seq = 0;
    let chains_buf = get_list_of_objects(
        data_type,
        seq,
        filter,
        if filter.is_some() {
            filter.unwrap().get_family()
        } else {
            ProtocolFamily::Unspec
            // this probably won't get you anything. usually programming error
        },
    )?;
    sock.send(&chains_buf).await?;

    // the kernel should return NLM_F_MULTI objects
    recv_and_process_async(
        sock,
        None,
        Some(&|buf: &[u8], working_data: &mut Accumulator| {
            debug!("Calling Object::deserialize()");
            cb(Object::deserialize(buf)?.0, working_data)
        }),
        working_data,
    )
    .await?;

    Ok(())
}
