use grammers_mtproto::{
    DequeBuffer,
    transport::{Error, Transport, UnpackedOffset},
};

/// TDLib's intermediate transport port. Quick acknowledgements are not requested by this adapter.
#[derive(Default)]
pub struct TdTransport {
    initialized: bool,
}
impl Transport for TdTransport {
    fn pack(&mut self, buffer: &mut DequeBuffer<u8>) {
        let header = td_rust::intermediate_header(buffer.len(), false)
            .expect("valid MTProto payload length");
        buffer.extend_front(&header);
        if !self.initialized {
            buffer.extend_front(&td_rust::INTERMEDIATE_TAG);
            self.initialized = true;
        }
    }
    fn unpack(&mut self, buffer: &mut [u8]) -> Result<UnpackedOffset, Error> {
        match td_rust::intermediate_frame(buffer) {
            Ok(td_rust::Frame::Packet { data, consumed }) => {
                if data.len() == 4 {
                    let status = i32::from_le_bytes(buffer[data.clone()].try_into().unwrap());
                    if status < 0 {
                        return Err(Error::BadStatus {
                            status: status.unsigned_abs(),
                        });
                    }
                }
                Ok(UnpackedOffset {
                    data_range: data,
                    next_offset: consumed,
                })
            }
            Ok(td_rust::Frame::QuickAck(_)) => Err(Error::BadLen { got: -1 }),
            Err(td_rust::Error::Incomplete) => Err(Error::MissingBytes),
            Err(td_rust::Error::Length(length)) => Err(Error::BadLen { got: length as i32 }),
            Err(_) => Err(Error::BadLen { got: 0 }),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_status_and_payload_never_panic_or_return_out_of_bounds_offsets() {
        let mut transport = TdTransport::default();
        let mut frame = 16u32.to_le_bytes().to_vec();
        frame.extend([7; 16]);
        for end in 0..20 {
            assert_eq!(
                transport.unpack(&mut frame[..end]),
                Err(Error::MissingBytes)
            );
        }
        assert_eq!(transport.unpack(&mut frame).unwrap().data_range, 4..20);
        let mut status = 4u32.to_le_bytes().to_vec();
        status.extend((-404i32).to_le_bytes());
        for end in 0..8 {
            assert_eq!(
                transport.unpack(&mut status[..end]),
                Err(Error::MissingBytes)
            );
        }
        assert_eq!(
            transport.unpack(&mut status),
            Err(Error::BadStatus { status: 404 })
        );
    }
}
