// Copyright 2023 Simo Sorce
// See LICENSE.txt file for terms

use core::fmt::Debug;
use data_encoding::BASE64;

use super::interface;
use interface::CK_RV;

use serde::{Serialize, Deserialize};
use serde_json::{Map, Value, Number};

pub trait Object {
    fn get_handle(&self) -> interface::CK_OBJECT_HANDLE;
    fn get_class(&self) -> interface::CK_OBJECT_CLASS;
}

macro_rules! object_constructor {
    ($name:ty) => {
        impl Object for $name {
            fn get_handle(&self) -> interface::CK_OBJECT_HANDLE {
                self.handle
            }

            fn get_class(&self) -> interface::CK_OBJECT_CLASS {
                self.class
            }
        }
    }
}

impl Debug for dyn Object {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Something!")
    }
}

// TODO: HW Feature Objects

macro_rules! bool_attribute {
    ($name:expr; from $map:expr; def $def:expr) => {
        match $map.get($name) {
            Some(Value::Bool(b)) => *b,
            _ => $def
        }
    }
}

macro_rules! str_attribute {
    ($name:expr; from $map:expr) => {
        match $map.get($name) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None
        }
    }
}

macro_rules! bytes_attribute {
    ($name:expr; from $map:expr) => {
        match $map.get($name) {
            Some(Value::String(s)) => {
                let len = match BASE64.decode_len(s.len()) {
                    Ok(l) => l,
                    Err(_e) => return None,
                };
                let mut output = vec![0; len];
                let _ = match BASE64.decode_mut(s.as_bytes(), &mut output) {
                    Ok(l) => l,
                    Err(_e) => return None,
                };
                Some(output)
            },
            _ => None
        }
    }
}

macro_rules! ulong_set_attribute {
    ($name:expr; $value:expr; into $map:expr) => {
        {
            let old = match $map.insert($name, Value::Number(Number::from($value))) {
                Some(o) => o,
                _ => return Err(interface::CKR_GENERAL_ERROR),
            };
            Ok(old)
        }
    }
}

macro_rules! string_set_attribute {
    ($name:expr; $value:expr; into $map:expr) => {
        {
            let old = match $map.insert($name, Value::String($value)) {
                Some(o) => o,
                _ => return Err(interface::CKR_GENERAL_ERROR),
            };
            Ok(old)
        }
    }
}

macro_rules! bool_set_attribute {
    ($name:expr; $value:expr; into $map:expr) => {
        {
            let old = match $map.insert($name, Value::Bool($value)) {
                Some(o) => o,
                _ => return Err(interface::CKR_GENERAL_ERROR),
            };
            Ok(old)
        }
    }
}

macro_rules! bytes_set_attribute {
    ($name:expr; $value:expr; into $map:expr) => {
        {
            let sval = BASE64.encode($value.as_ref());
            let old = match $map.insert($name, Value::String(sval)) {
                Some(o) => o,
                _ => return Err(interface::CKR_GENERAL_ERROR),
            };
            Ok(old)
        }
    }
}

macro_rules! with {
    ($str:expr) => {
        {
            $str.to_string()
        }
    }
}

pub trait Storage {
    fn is_token(&self) -> bool {
        false
    }
    fn is_private(&self) -> bool;
    fn is_modifiable(&self) -> bool {
        true
    }
    fn is_copyable(&self) -> bool {
        true
    }
    fn is_destroyable(&self) -> bool {
        true
    }
    fn get_label(&self) -> Option<String> {
        None
    }
    fn get_unique_id(&self) -> Option<String> {
        None
    }
    fn get_attr_as_bytes(&self, s: String) -> Option<Vec<u8>> {
        None
    }
    fn set_attr_from_ulong(&mut self, s: String, u: interface::CK_ULONG) -> Result<Value, CK_RV> {
        Err(interface::CKR_GENERAL_ERROR)
    }
    fn set_attr_from_string(&mut self, s: String, v: String) -> Result<Value, CK_RV> {
        Err(interface::CKR_GENERAL_ERROR)
    }
    fn set_attr_from_bool(&mut self, s: String, b: bool) -> Result<Value, CK_RV> {
        Err(interface::CKR_GENERAL_ERROR)
    }
    fn set_attr_from_bytes(&mut self, s: String, u: Vec<u8>) -> Result<Value, CK_RV> {
        Err(interface::CKR_GENERAL_ERROR)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct KeyObject {
    handle: interface::CK_OBJECT_HANDLE,
    class: interface::CK_OBJECT_CLASS,
    key_type: interface::CK_KEY_TYPE,
    attributes: Map<String, Value>,
}

impl Storage for KeyObject {
    fn is_token(&self) -> bool {
        bool_attribute!("CKA_TOKEN"; from self.attributes; def false)
    }
    fn is_private(&self) -> bool {
        bool_attribute!("CKA_PRIVATE"; from self.attributes; def true)
    }
    fn is_modifiable(&self) -> bool {
        bool_attribute!("CKA_MODIFIABLE"; from self.attributes; def true)
    }
    fn is_destroyable(&self) -> bool {
        bool_attribute!("CKA_DESTROYABLE"; from self.attributes; def false)
    }
    fn get_label(&self) -> Option<String> {
        str_attribute!("CKA_LABEL"; from self.attributes)
    }
    fn get_unique_id(&self) -> Option<String> {
        str_attribute!("CKA_ID"; from self.attributes)
    }
    fn get_attr_as_bytes(&self, s: String) -> Option<Vec<u8>> {
        bytes_attribute!(&s; from self.attributes)
    }
    fn set_attr_from_ulong(&mut self, s: String, u: interface::CK_ULONG) -> Result<Value, CK_RV> {
        ulong_set_attribute!(s; u; into self.attributes)
    }
    fn set_attr_from_string(&mut self, s: String, v: String) -> Result<Value, CK_RV> {
        string_set_attribute!(s; v; into self.attributes)
    }
    fn set_attr_from_bool(&mut self, s: String, b: bool) -> Result<Value, CK_RV> {
        bool_set_attribute!(s; b; into self.attributes)
    }
    fn set_attr_from_bytes(&mut self, s: String, u: Vec<u8>) -> Result<Value, CK_RV> {
        bytes_set_attribute!(s; u; into self.attributes)
    }
}
object_constructor!(KeyObject);

impl KeyObject {
    pub fn new() -> KeyObject {
        KeyObject {
            handle: 0,
            class: interface::CKO_PUBLIC_KEY,
            key_type: interface::CKK_RSA,
            attributes: Map::new(),
        }
    }

    pub fn test_object() -> KeyObject {
        let mut o = KeyObject {
            handle: 1234,
            class: interface::CKO_PUBLIC_KEY,
            key_type: interface::CKK_RSA,
            attributes: Map::new(),
        };

        o.set_attr_from_bool("CKA_TOKEN".to_string(), true);
        o.set_attr_from_bool(with!("CKA_PRIVATE"), false);
        o.set_attr_from_bool(with!("CKA_MODIFIABLE"), false);
        o.set_attr_from_bool(with!("CKA_DESTROYABLE"), false);
        o.set_attr_from_string(with!("CKA_LABEL"), with!("Test RSA Key"));
        o.set_attr_from_bytes(with!("CKA_ID"), b"\x01".to_vec());
        o.set_attr_from_bytes(with!("CKA_MODULUS"), b"\x01\x02\x03\x04\x05\x06\x07\x08".to_vec());
        o.set_attr_from_bytes(with!("CKA_PUBLIC_EXPONENT"), b"\x01\x00\x01".to_vec());

        o
    }
}
