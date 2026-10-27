// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Sans-IO state that a connection needs to hold.

use std::io;

use grammers_tl_types as tl;
use tracing::{debug, trace};

use crate::DequeBuffer;
use crate::authentication::Finished;
use crate::mtp::{self, Deserialization, DeserializeError, Encrypted, Mtp, Plain};
use crate::transport::Transport;
use crate::{MsgId, transport};

/// The maximum data that we're willing to send or receive at once.
///
/// By having a fixed-size buffer, we can avoid unnecessary allocations
/// and trivially prevent allocating more than this limit if we ever
/// received invalid data.
///
/// Telegram will close the connection with roughly a megabyte of data,
/// so to account for the transports' own overhead, we add a few extra
/// kilobytes to the maximum data size.
const MAXIMUM_DATA: usize = (1024 * 1024) + (8 * 1024);

/// How much leading space should be reserved in a buffer to avoid moving memory.
const LEADING_BUFFER_SPACE: usize = mtp::MAX_TRANSPORT_HEADER_LEN
    + mtp::ENCRYPTED_PACKET_HEADER_LEN
    + mtp::PLAIN_PACKET_HEADER_LEN
    + mtp::MESSAGE_CONTAINER_HEADER_LEN;

/// Glues together the transport and MTP state, and coordinates them to manage
/// the I/O buffers to send packed requests and iterate unpacked responses.
pub struct State<T: Transport, M: Mtp> {
    pub transport: T,
    pub mtp: M,
    read_buffer: Vec<u8>,
    read_head: usize,
    read_tail: usize,
    write_buffer: DequeBuffer<u8>,
    write_head: usize,
    write_ready: bool,
}

impl<T: Transport, M: Mtp> State<T, M> {
    pub fn new(transport: T, mtp: M) -> Self {
        Self {
            transport,
            mtp,
            read_buffer: vec![0; MAXIMUM_DATA],
            read_head: 0,
            read_tail: 0,
            write_buffer: DequeBuffer::with_capacity(MAXIMUM_DATA, LEADING_BUFFER_SPACE),
            write_head: 0,
            write_ready: false,
        }
    }

    /// Returns `true` if requests can be buffered for sending.
    pub fn can_buffer_requests(&self) -> bool {
        !self.write_ready
    }

    /// Buffer a request's body to be sent.
    ///
    /// If the request can fit, its assigned message identifier is returned.
    ///
    /// Buffering will fail if not [`Self::can_buffer_requests`].
    pub fn buffer_request(&mut self, body: &[u8]) -> Option<MsgId> {
        if self.write_ready {
            return None;
        }

        // TODO make mtp itself use BytesMut to avoid copies
        if let Some(msg_id) = self.mtp.push(&mut self.write_buffer, body) {
            assert!(body.len() >= 4);
            let req_id = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
            debug!(
                req_id = format_args!("{:x}", req_id),
                req_name = tl::name_for_id(req_id),
                ?msg_id,
                "serialized request",
            );
            Some(msg_id)
        } else {
            None
        }
    }

    /// Pack the outgoing buffer to make it ready for sending over the network.
    ///
    /// This should be done after having [`Self::buffer_request`] with as many
    /// requests as could fit, although a sendable buffer may be produced even
    /// if no user requests were buffered.
    ///
    /// Packing can only be done if [`Self::can_buffer_requests`].
    pub fn pack_outgoing_buffer(&mut self) -> Option<MsgId> {
        if self.write_ready {
            return None;
        }
        if let Some(container_msg_id) = self.mtp.finalize(&mut self.write_buffer) {
            self.transport.pack(&mut self.write_buffer);
            self.write_ready = true;
            Some(container_msg_id)
        } else {
            None
        }
    }

    /// Return a reference to the writable part of the read buffer for
    /// receiving, and the readable part of the write buffer for sending.
    ///
    /// If there is nothing to send, the write buffer will be zero sized.
    pub fn io_buffers(&mut self) -> (&mut [u8], &[u8]) {
        (
            &mut self.read_buffer[self.read_tail..],
            if self.write_ready {
                &self.write_buffer[self.write_head..]
            } else {
                &self.write_buffer[..0]
            },
        )
    }

    /// Mark `n` received bytes as being ready to be processed by
    /// [`Self::unpack_next`].
    ///
    /// Reading zero bytes from the network signals that the
    /// connection was reset. Error state is returned in that case.
    pub fn handle_net_read(&mut self, n: usize) -> Result<(), io::Error> {
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "read 0 bytes",
            ));
        }

        self.read_tail += n;
        trace!("read {} bytes from the network", n);
        trace!("trying to unpack buffer of {} bytes...", self.read_tail);
        Ok(())
    }

    /// Mark `n` sent bytes as having been flushed from the write buffer.
    ///
    /// Returns `true` if all data has been sent and
    /// the buffer is ready to be written into again.
    pub fn handle_net_write(&mut self, n: usize) -> bool {
        self.write_head += n;
        trace!(
            "written {} bytes to the network ({}/{})",
            n,
            self.write_head,
            self.write_buffer.len()
        );
        assert!(self.write_head <= self.write_buffer.len());
        if self.write_head != self.write_buffer.len() {
            return false;
        }

        self.write_buffer.clear();
        self.write_head = 0;
        self.write_ready = false;
        true
    }

    /// Unpack the next packet from the read buffer.
    ///
    /// If there is not enough data to unpack anything, `None` is returned.
    ///
    /// This should be called in a loop until `None` is returned.
    pub fn unpack_next(&mut self) -> Option<Result<Vec<Deserialization>, DeserializeError>> {
        if self.read_head != self.read_tail {
            match self
                .transport
                .unpack(&mut self.read_buffer[self.read_head..self.read_tail])
            {
                Ok(offset) => {
                    debug!("deserializing valid transport packet...");
                    let result = self
                        .mtp
                        .deserialize(&mut self.read_buffer[self.read_head..][offset.data_range]);

                    self.read_head += offset.next_offset;
                    return Some(result);
                }
                Err(transport::Error::MissingBytes) => {}
                Err(err) => return Some(Err(DeserializeError::Transport(err))),
            }
        }

        self.read_buffer
            .copy_within(self.read_head..self.read_tail, 0);

        self.read_tail -= self.read_head;
        self.read_head = 0;
        None
    }
}

impl<T: Transport> State<T, Plain> {
    /// Convert Plain state into Encrypted with the final authentication step.
    pub fn into_encrypted(self, auth: Finished) -> State<T, Encrypted> {
        State {
            transport: self.transport,
            mtp: mtp::Encrypted::build()
                .time_offset(auth.time_offset)
                .first_salt(auth.first_salt)
                .finish(auth.auth_key),
            read_buffer: self.read_buffer,
            read_head: self.read_head,
            read_tail: self.read_tail,
            write_buffer: self.write_buffer,
            write_head: self.write_head,
            write_ready: self.write_ready,
        }
    }
}
