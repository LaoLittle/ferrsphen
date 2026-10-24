use super::{Empty, Entry, IntegerByteSequence, Map, Record, Sequence, Value, value::Kind};
use anyhow::{Context, Result, bail, ensure};
use prost::Message as _;
use serde::{Serialize, de::DeserializeOwned};
use serde_value::Value as Data;
use std::collections::BTreeMap;

const MAGIC: &[u8] = b"\xffFSPB\x01\r\n";
const VERSION: u32 = 1;
const MAX_DEPTH: usize = 32;

pub(crate) fn is_protobuf(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}
pub(crate) fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    encode_data(serde_value::to_value(value)?)
}
fn encode_data(value: Data) -> Result<Vec<u8>> {
    let record = Record {
        schema_version: VERSION,
        value: Some(to_proto(value, 0)?),
    };
    let mut output = Vec::with_capacity(MAGIC.len() + record.encoded_len());
    output.extend_from_slice(MAGIC);
    record.encode(&mut output)?;
    Ok(output)
}
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    if let Some(bytes) = bytes.strip_prefix(MAGIC) {
        let record = Record::decode(bytes).context("读取 protobuf 数据库记录失败")?;
        ensure!(
            record.schema_version == VERSION,
            "不支持的数据库版本：{}",
            record.schema_version
        );
        let data = from_proto(record.value.context("数据库记录缺少值")?, 0)?;
        T::deserialize(serde_value::ValueDeserializer::<
            serde_value::DeserializerError,
        >::new(data))
        .map_err(Into::into)
    } else {
        read_legacy(bytes)
    }
}
fn read_legacy<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let mut input = bytes;
    let value = ciborium::from_reader(&mut input).context("读取旧数据库记录失败")?;
    ensure!(input.is_empty(), "旧数据库记录存在多余数据");
    Ok(value)
}
pub(crate) fn transcode_legacy(bytes: &[u8]) -> Result<Vec<u8>> {
    encode_data(read_legacy(bytes)?)
}
fn wide_octet(value: &Data) -> Option<(u8, u32)> {
    match value {
        Data::U16(v) => u8::try_from(*v).ok().map(|byte| (byte, 16)),
        Data::U32(v) => u8::try_from(*v).ok().map(|byte| (byte, 32)),
        Data::U64(v) => u8::try_from(*v).ok().map(|byte| (byte, 64)),
        _ => None,
    }
}
fn wide_octet_width(values: &[Data]) -> Option<u32> {
    let (_, bits) = wide_octet(values.first()?)?;
    values
        .iter()
        .all(|v| wide_octet(v).is_some_and(|(_, width)| width == bits))
        .then_some(bits)
}
fn to_proto(value: Data, depth: usize) -> Result<Value> {
    ensure!(depth <= MAX_DEPTH, "数据库记录嵌套过深");
    let kind = match value {
        Data::Bool(v) => Kind::Boolean(v),
        Data::U8(v) => Kind::U8(v.into()),
        Data::U16(v) => Kind::U16(v.into()),
        Data::U32(v) => Kind::U32(v),
        Data::U64(v) => Kind::U64(v),
        Data::I8(v) => Kind::I8(v.into()),
        Data::I16(v) => Kind::I16(v.into()),
        Data::I32(v) => Kind::I32(v),
        Data::I64(v) => Kind::I64(v),
        Data::F32(v) => Kind::F32(v),
        Data::F64(v) => Kind::F64(v),
        Data::Char(v) => Kind::Character(v.to_string()),
        Data::String(v) => Kind::String(v),
        Data::Unit => Kind::Unit(Empty {}),
        Data::Option(None) => Kind::None(Empty {}),
        Data::Option(Some(v)) => Kind::Some(Box::new(to_proto(*v, depth + 1)?)),
        Data::Newtype(v) => Kind::Newtype(Box::new(to_proto(*v, depth + 1)?)),
        Data::Bytes(v) => Kind::Bytes(v),
        Data::Seq(values)
            if !values.is_empty() && values.iter().all(|v| matches!(v, Data::U8(_))) =>
        {
            Kind::ByteSequence(
                values
                    .into_iter()
                    .map(|v| {
                        if let Data::U8(v) = v {
                            v
                        } else {
                            unreachable!()
                        }
                    })
                    .collect(),
            )
        }
        Data::Seq(values) if wide_octet_width(&values).is_some() => {
            let element_bits = wide_octet_width(&values).unwrap();
            let values = values.iter().map(|v| wide_octet(v).unwrap().0).collect();
            Kind::IntegerByteSequence(IntegerByteSequence {
                values,
                element_bits,
            })
        }
        Data::Seq(values) => Kind::Sequence(Sequence {
            values: values
                .into_iter()
                .map(|v| to_proto(v, depth + 1))
                .collect::<Result<_>>()?,
        }),
        Data::Map(entries) => Kind::Map(Map {
            entries: entries
                .into_iter()
                .map(|(key, value)| {
                    Ok(Entry {
                        key: Some(to_proto(key, depth + 1)?),
                        value: Some(to_proto(value, depth + 1)?),
                    })
                })
                .collect::<Result<_>>()?,
        }),
    };
    Ok(Value { kind: Some(kind) })
}
fn from_proto(value: Value, depth: usize) -> Result<Data> {
    ensure!(depth <= MAX_DEPTH, "数据库记录嵌套过深");
    Ok(match value.kind.context("数据库值缺少类型")? {
        Kind::Boolean(v) => Data::Bool(v),
        Kind::U8(v) => Data::U8(v.try_into()?),
        Kind::U16(v) => Data::U16(v.try_into()?),
        Kind::U32(v) => Data::U32(v),
        Kind::U64(v) => Data::U64(v),
        Kind::I8(v) => Data::I8(v.try_into()?),
        Kind::I16(v) => Data::I16(v.try_into()?),
        Kind::I32(v) => Data::I32(v),
        Kind::I64(v) => Data::I64(v),
        Kind::F32(v) => Data::F32(v),
        Kind::F64(v) => Data::F64(v),
        Kind::Character(v) => {
            let mut chars = v.chars();
            let c = chars.next().context("数据库字符为空")?;
            ensure!(chars.next().is_none(), "无效的数据库字符");
            Data::Char(c)
        }
        Kind::String(v) => Data::String(v),
        Kind::Unit(_) => Data::Unit,
        Kind::None(_) => Data::Option(None),
        Kind::Some(v) => Data::Option(Some(Box::new(from_proto(*v, depth + 1)?))),
        Kind::Newtype(v) => Data::Newtype(Box::new(from_proto(*v, depth + 1)?)),
        Kind::Bytes(v) => Data::Bytes(v),
        Kind::ByteSequence(v) => Data::Seq(v.into_iter().map(Data::U8).collect()),
        Kind::IntegerByteSequence(v) => {
            ensure!(
                matches!(v.element_bits, 16 | 32 | 64),
                "无效的数据库整数位宽"
            );
            Data::Seq(
                v.values
                    .into_iter()
                    .map(|byte| match v.element_bits {
                        16 => Data::U16(byte.into()),
                        32 => Data::U32(byte.into()),
                        _ => Data::U64(byte.into()),
                    })
                    .collect(),
            )
        }
        Kind::Sequence(v) => Data::Seq(
            v.values
                .into_iter()
                .map(|v| from_proto(v, depth + 1))
                .collect::<Result<_>>()?,
        ),
        Kind::Map(v) => {
            let mut values = BTreeMap::new();
            for entry in v.entries {
                let key = from_proto(entry.key.context("数据库映射缺少键")?, depth + 1)?;
                let value = from_proto(entry.value.context("数据库映射缺少值")?, depth + 1)?;
                if values.insert(key, value).is_some() {
                    bail!("数据库映射包含重复键");
                }
            }
            Data::Map(values)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{MAGIC, MAX_DEPTH, decode, encode, is_protobuf};
    use crate::proto::{Record, Value, value::Kind};
    use prost::Message as _;
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Sample {
        account: u64,
        negative: i64,
        number: f64,
        text: String,
        bytes: Vec<u8>,
        empty_bytes: Vec<u8>,
        absent: Option<u64>,
        zero: Option<u64>,
        values: BTreeMap<u64, String>,
    }
    #[test]
    fn protobuf_preserves_integer_limits_optional_zero_binary_and_non_string_map_keys() {
        let sample = Sample {
            account: u64::MAX,
            negative: i64::MIN,
            number: -0.25,
            text: "消息🌙".into(),
            bytes: (0..=255).collect(),
            empty_bytes: vec![],
            absent: None,
            zero: Some(0),
            values: BTreeMap::from([(u64::MAX, "末尾".into())]),
        };
        let encoded = encode(&sample).unwrap();
        assert!(is_protobuf(&encoded));
        let record = Record::decode(&encoded[MAGIC.len()..]).unwrap();
        assert_eq!(record.schema_version, 1);
        assert_eq!(decode::<Sample>(&encoded).unwrap(), sample);
        let mut legacy = Vec::new();
        ciborium::into_writer(&sample, &mut legacy).unwrap();
        let migrated = super::transcode_legacy(&legacy).unwrap();
        assert_eq!(decode::<Sample>(&migrated).unwrap(), sample);
        let binary = encode(&sample.bytes).unwrap();
        assert!(binary.len() < 280);
        assert!(matches!(
            Record::decode(&binary[MAGIC.len()..])
                .unwrap()
                .value
                .unwrap()
                .kind,
            Some(Kind::ByteSequence(_))
        ));
    }
    #[test]
    fn legacy_byte_arrays_migrate_compactly_without_losing_integer_widths() {
        let source = (0..=255u8).collect::<Vec<_>>();
        let mut legacy = Vec::new();
        ciborium::into_writer(&source, &mut legacy).unwrap();
        let migrated = super::transcode_legacy(&legacy).unwrap();
        assert!(migrated.len() < 300);
        assert_eq!(decode::<Vec<u8>>(&migrated).unwrap(), source);
        let wide = serde_value::Value::Seq((0..=255u64).map(serde_value::Value::U64).collect());
        let bytes = encode(&wide).unwrap();
        assert!(bytes.len() < 300);
        assert_eq!(decode::<serde_value::Value>(&bytes).unwrap(), wide);
    }
    #[test]
    fn corrupt_or_future_protobuf_is_not_reinterpreted_as_legacy_cbor() {
        for record in [
            Record {
                schema_version: 2,
                value: Some(Value {
                    kind: Some(Kind::U64(1)),
                }),
            },
            Record {
                schema_version: 1,
                value: None,
            },
            Record {
                schema_version: 1,
                value: Some(Value { kind: None }),
            },
            Record {
                schema_version: 1,
                value: Some(Value {
                    kind: Some(Kind::U8(256)),
                }),
            },
        ] {
            let mut bytes = MAGIC.to_vec();
            bytes.extend(record.encode_to_vec());
            assert!(decode::<u64>(&bytes).is_err());
        }
        let mut truncated = encode(&"a long message").unwrap();
        truncated.pop();
        assert!(decode::<String>(&truncated).is_err());
    }
    #[test]
    fn supported_nesting_round_trips_and_excessive_nesting_is_rejected() {
        let mut value = serde_value::Value::U64(1);
        for _ in 0..MAX_DEPTH {
            value = serde_value::Value::Map(BTreeMap::from([(
                serde_value::Value::String("value".into()),
                value,
            )]));
        }
        let result = encode(&value).and_then(|bytes| decode::<serde_value::Value>(&bytes));
        assert_eq!(result.unwrap(), value);
        value = serde_value::Value::Seq(vec![value]);
        assert!(encode(&value).is_err());
    }
}
