use crate::proto::ferrsphen::message_presentation::{Envelope, ReactionAttributes, TextReserve};
use atri_qq::messages::{MessageElement, OpaqueScope, validate_metadata};
use prost::Message;

// Decode only when re-encoding reproduces the bytes exactly, so unknown or
// noncanonical data stays visible.
fn exact<M: Message + Default>(bytes: &[u8]) -> Option<M> {
    let value = M::decode(bytes).ok()?;
    (value.encode_to_vec() == bytes).then_some(value)
}

pub fn is_visible(element: &MessageElement, elements: &[MessageElement]) -> bool {
    let MessageElement::Opaque(record) = element else {
        return true;
    };
    if validate_metadata(record).is_ok() {
        return false;
    }
    if record.scope != OpaqueScope::Element {
        return true;
    }
    let Some(envelope) = exact::<Envelope>(&record.payload) else {
        return true;
    };
    let auxiliary = match record.field {
        1 => {
            !envelope.text.is_empty()
                && envelope.reply.is_empty()
                && envelope.common.is_empty()
                && envelope.text.iter().all(|text| {
                    text.reserve.as_ref().is_none_or(|bytes| {
                        exact::<TextReserve>(bytes)
                            .is_some_and(|reserve| reserve.field11 == Some(0))
                    })
                })
        }
        45 => !envelope.reply.is_empty() && envelope.text.is_empty() && envelope.common.is_empty(),
        // Service 38 attributes accompany image and text content.
        // Keep standalone or unrecognized instances inspectable.
        53 => {
            elements.iter().any(|element| match element {
                MessageElement::Text(text) => !text.is_empty(),
                MessageElement::Image(_) | MessageElement::Mention(_) => true,
                MessageElement::Opaque(_) => {
                    crate::expressions::Face::from_element(element).is_some()
                }
                _ => false,
            }) && !envelope.common.is_empty()
                && envelope.text.is_empty()
                && envelope.reply.is_empty()
                && envelope.common.iter().all(|common| {
                    common.service == Some(38)
                        && common.business == Some(1)
                        && exact::<ReactionAttributes>(&common.payload)
                            .is_some_and(|data| data.reaction.is_some() || data.reply.is_some())
                })
        }
        _ => false,
    };
    !auxiliary
}

pub fn visible_elements(elements: &[MessageElement]) -> impl Iterator<Item = &MessageElement> {
    elements
        .iter()
        .filter(|element| is_visible(element, elements))
}

pub fn single_face(elements: &[MessageElement]) -> Option<crate::expressions::Face> {
    let mut visible = visible_elements(elements);
    let element = visible.next()?;
    if visible.next().is_some() {
        return None;
    }
    crate::expressions::Face::from_element(element)
}

#[cfg(test)]
mod tests {
    use super::*;
    use atri_qq::{
        media::{MediaConversation, MediaKind, MediaReference},
        messages::OpaqueElement,
    };

    fn opaque(field: u32, hex: &str) -> MessageElement {
        MessageElement::Opaque(OpaqueElement {
            scope: OpaqueScope::Element,
            field,
            payload: hex::decode(hex).unwrap(),
        })
    }
    #[test]
    fn only_a_single_visible_qq_face_uses_the_large_presentation() {
        let face = crate::expressions::Face::new(14).element();
        assert_eq!(single_face(std::slice::from_ref(&face)).unwrap().id, 14);
        let decorated = vec![
            face.clone(),
            opaque(53, "aa030f082612090a070a0508021080091801"),
        ];
        assert_eq!(single_face(&decorated).unwrap().id, 14);
        let reply = MessageElement::Reply(atri_qq::messages::Reply {
            sequences: vec![1],
            server_sequence: None,
            message_id: 1,
            sender_uin: 7,
            sender_uid: String::new(),
            time: 0,
            elements: vec![],
        });
        assert!(single_face(&[reply, face.clone()]).is_none());
        for elements in [
            vec![],
            vec![face.clone(), face.clone()],
            vec![MessageElement::Text("text".into()), face.clone()],
            vec![MessageElement::Text(" ".into()), face.clone()],
            vec![face, opaque(53, "aa0303082718")],
        ] {
            assert!(single_face(&elements).is_none());
        }
    }
    #[test]
    fn text_and_reply_attributes_do_not_add_other_message_to_preview() {
        let elements = vec![
            MessageElement::Text("hello".into()),
            opaque(1, "0a051a03000100"),
            opaque(1, "0a0462025800"),
            opaque(45, "ea02025000"),
        ];
        assert_eq!(visible_elements(&elements).count(), 1);
        assert_eq!(crate::model::summary(&elements), "hello");
    }
    #[test]
    fn image_reaction_sidecar_preserves_image_only_presentation() {
        let mut elements = vec![
            MessageElement::Image(MediaReference::new(
                MediaKind::Image,
                MediaConversation::Group { uin: 42 },
            )),
            opaque(53, "aa030f082612090a070a0508041091091801"),
        ];
        assert_eq!(visible_elements(&elements).count(), 1);
        assert!(!crate::model::summary(&elements).contains("其他消息"));
        elements.remove(0);
        assert_eq!(
            visible_elements(&elements).count(),
            1,
            "standalone data must remain inspectable"
        );
    }
    #[test]
    fn text_attributes_do_not_create_an_extra_bubble_or_hide_unknown_payloads() {
        for payload in [
            "aa030f082612090a070a0508021080091801",
            "aa030f082612090a070a0508021099091801",
            "aa030f082612090a070a050802109b091801",
            "aa030f082612090a070a050806109a091801",
            "aa030f082612090a070a05080a109c091801",
            "aa030f082612090a070a05080c1098091801",
        ] {
            let elements = vec![MessageElement::Text("hello".into()), opaque(53, payload)];
            assert_eq!(visible_elements(&elements).count(), 1);
            assert_eq!(crate::model::summary(&elements), "hello");
            assert!(is_visible(&elements[1], &elements[1..]));
        }
        let elements = vec![
            MessageElement::Text("hello".into()),
            opaque(53, "aa0311082612090a070a05080210800918012001"),
        ];
        assert_eq!(visible_elements(&elements).count(), 2);
    }
    #[test]
    fn unknown_content_malformed_data_and_faces_remain_visible() {
        for element in [
            opaque(1, "0a03120178"),
            opaque(1, "0a03620101"),
            opaque(1, "0a040a026869"),
            opaque(45, "ea02020801"),
            opaque(1, "ff"),
            opaque(53, "aa030f082712090a070a0508041091091801"),
            crate::expressions::Face::new(14).element(),
        ] {
            assert!(is_visible(&element, std::slice::from_ref(&element)));
        }
    }
}
