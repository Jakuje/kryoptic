use crate::attribute::string_to_ck_date;
use crate::attribute::{AttrType, Attribute};
use crate::error::{Error, Result};
use crate::interface::*;
use crate::misc::copy_sized_string;
use crate::object::Object;
use crate::storage::aci;
use crate::storage::format;
use crate::storage::StorageTokenInfo;

use data_encoding::BASE64;
use serde::{Deserialize, Serialize};
use serde_json::{from_reader, to_string_pretty, Map, Number, Value};

fn uninit(e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        Error::ck_rv_from_error(CKR_CRYPTOKI_NOT_INITIALIZED, e)
    } else {
        Error::other_error(e)
    }
}

fn to_json_value(a: &Attribute) -> Value {
    match a.get_attrtype() {
        AttrType::BoolType => match a.to_bool() {
            Ok(b) => Value::Bool(b),
            Err(_) => Value::Null,
        },
        AttrType::NumType => match a.to_ulong() {
            Ok(l) => Value::Number(Number::from(l)),
            Err(_) => Value::Null,
        },
        AttrType::StringType => match a.to_string() {
            Ok(s) => Value::String(s),
            Err(_) => Value::String(BASE64.encode(a.get_value())),
        },
        AttrType::BytesType => Value::String(BASE64.encode(a.get_value())),
        AttrType::DateType => match a.to_date_string() {
            Ok(d) => Value::String(d),
            Err(_) => Value::String(String::new()),
        },
        AttrType::IgnoreType => Value::Null,
        AttrType::DenyType => Value::Null,
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonObject {
    attributes: Map<String, Value>,
}

impl JsonObject {
    pub fn from_object(o: &Object) -> JsonObject {
        let mut jo = JsonObject {
            attributes: Map::new(),
        };
        for a in o.get_attributes() {
            jo.attributes.insert(a.name(), to_json_value(a));
        }
        jo
    }
    fn from_user(uid: &str, info: &aci::StorageAuthInfo) -> JsonObject {
        let mut ju = JsonObject {
            attributes: Map::new(),
        };
        ju.attributes
            .insert("name".to_string(), Value::String(uid.to_string()));
        ju.attributes
            .insert("default_pin".to_string(), Value::Bool(info.default_pin));
        ju.attributes.insert(
            "attempts".to_string(),
            Value::Number(Number::from(info.cur_attempts)),
        );
        if let Some(data) = &info.user_data {
            ju.attributes.insert(
                "data".to_string(),
                Value::String(BASE64.encode(data.as_slice())),
            );
        }
        ju
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonTokenInfo {
    label: String,
    manufacturer: String,
    model: String,
    serial: String,
    flags: CK_ULONG,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonObjects {
    objects: Vec<JsonObject>,
    users: Option<Vec<JsonObject>>,
    token: Option<JsonTokenInfo>,
}

impl JsonObjects {
    pub fn load(filename: &str) -> Result<JsonObjects> {
        match std::fs::File::open(filename) {
            Ok(f) => Ok(from_reader::<std::fs::File, JsonObjects>(f)?),
            Err(e) => Err(uninit(e)),
        }
    }

    pub fn prime_store(
        &self,
        store: &mut Box<dyn format::StorageRaw>,
    ) -> Result<()> {
        if let Some(t) = &self.token {
            let mut info = StorageTokenInfo {
                label: [0; 32],
                manufacturer: [0; 32],
                model: [0; 16],
                serial: [0; 16],
                flags: 0,
            };
            copy_sized_string(t.label.as_bytes(), &mut info.label);
            copy_sized_string(
                t.manufacturer.as_bytes(),
                &mut info.manufacturer,
            );
            copy_sized_string(t.model.as_bytes(), &mut info.model);
            copy_sized_string(t.serial.as_bytes(), &mut info.serial);
            info.flags = t.flags;
            store.store_token_info(&info)?;
        }
        if let Some(users) = &self.users {
            for ju in users {
                let mut uid = String::new();
                let mut info = aci::StorageAuthInfo::default();
                for (key, val) in &ju.attributes {
                    match key.as_str() {
                        "name" => match val.as_str() {
                            Some(s) => uid.push_str(s),
                            None => return Err(CKR_DEVICE_ERROR)?,
                        },
                        "default_pin" => match val.as_bool() {
                            Some(b) => info.default_pin = b,
                            None => return Err(CKR_DEVICE_ERROR)?,
                        },
                        "attempts" => match val.as_u64() {
                            Some(u) => {
                                info.cur_attempts = CK_ULONG::try_from(u)?
                            }
                            None => return Err(CKR_DEVICE_ERROR)?,
                        },
                        "data" => match val.as_str() {
                            Some(s) => {
                                let len = match BASE64.decode_len(s.len()) {
                                    Ok(l) => l,
                                    Err(_) => return Err(CKR_DEVICE_ERROR)?,
                                };
                                let mut v = vec![0; len];
                                match BASE64.decode_mut(s.as_bytes(), &mut v) {
                                    Ok(l) => v.resize(l, 0),
                                    Err(_) => return Err(CKR_DEVICE_ERROR)?,
                                }
                                info.user_data = Some(v);
                            }
                            None => return Err(CKR_DEVICE_ERROR)?,
                        },
                        _ => (), /* ignore unknown */
                    }
                }
                store.store_user(uid.as_str(), &info)?;
            }
        }
        for jo in &self.objects {
            let mut obj = Object::new();
            for (key, val) in &jo.attributes {
                let (id, atype) = AttrType::attr_name_to_id_type(key)?;
                let attr = match atype {
                    AttrType::BoolType => match val.as_bool() {
                        Some(b) => Attribute::from_bool(id, b),
                        None => return Err(CKR_ATTRIBUTE_VALUE_INVALID)?,
                    },
                    AttrType::NumType => match val.as_u64() {
                        Some(n) => Attribute::from_u64(id, n),
                        None => return Err(CKR_ATTRIBUTE_VALUE_INVALID)?,
                    },
                    AttrType::StringType => match val.as_str() {
                        Some(s) => Attribute::from_string(id, s.to_string()),
                        None => return Err(CKR_ATTRIBUTE_VALUE_INVALID)?,
                    },
                    AttrType::BytesType => match val.as_str() {
                        Some(s) => {
                            let len = match BASE64.decode_len(s.len()) {
                                Ok(l) => l,
                                Err(_) => return Err(CKR_GENERAL_ERROR)?,
                            };
                            let mut v = vec![0; len];
                            match BASE64.decode_mut(s.as_bytes(), &mut v) {
                                Ok(l) => {
                                    Attribute::from_bytes(id, v[0..l].to_vec())
                                }
                                Err(_) => return Err(CKR_GENERAL_ERROR)?,
                            }
                        }
                        None => return Err(CKR_ATTRIBUTE_VALUE_INVALID)?,
                    },
                    AttrType::DateType => match val.as_str() {
                        Some(s) => {
                            if s.len() == 0 {
                                /* special case for default empty value */
                                Attribute::from_date_bytes(id, Vec::new())
                            } else {
                                Attribute::from_date(id, string_to_ck_date(&s)?)
                            }
                        }
                        None => return Err(CKR_ATTRIBUTE_VALUE_INVALID)?,
                    },
                    AttrType::DenyType => continue,
                    AttrType::IgnoreType => continue,
                };

                obj.set_attr(attr)?;
            }
            store.store_obj(obj)?;
        }
        Ok(())
    }

    pub fn from_store(store: &mut Box<dyn format::StorageRaw>) -> JsonObjects {
        let objs = store.search(&[]).unwrap();
        let mut jobjs = Vec::with_capacity(objs.len());
        for o in objs {
            if !o.is_token() {
                continue;
            }
            jobjs.push(JsonObject::from_object(&o));
        }

        let info = store.fetch_token_info().unwrap();
        let jtoken = JsonTokenInfo {
            label: String::from_utf8(info.label.to_vec()).unwrap(),
            manufacturer: String::from_utf8(info.manufacturer.to_vec())
                .unwrap(),
            model: String::from_utf8(info.model.to_vec()).unwrap(),
            serial: String::from_utf8(info.serial.to_vec()).unwrap(),
            flags: info.flags,
        };

        let mut jusers = Vec::new();
        for id in [format::SO_ID, format::USER_ID] {
            match store.fetch_user(id) {
                Ok(u) => jusers.push(JsonObject::from_user(id, &u)),
                Err(_) => (),
            }
        }

        JsonObjects {
            objects: jobjs,
            users: Some(jusers),
            token: Some(jtoken),
        }
    }

    pub fn save(&self, filename: &str) -> Result<()> {
        let jstr = match to_string_pretty(&self) {
            Ok(j) => j,
            Err(e) => return Err(Error::other_error(e)),
        };
        match std::fs::write(filename, jstr) {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::other_error(e)),
        }
    }
}
