// Copyright 2023 Simo Sorce
// See LICENSE.txt file for terms

use std::collections::HashMap;
use std::vec::Vec;

use serde::{Serialize, Deserialize};
use serde_json;

use super::interface;
use super::attribute;
use super::session;
use super::object;
use super::error;

use interface::*;
use session::Session;
use object::Object;
use error::{KResult, KError};
use super::{err_rv, err_not_found};

use getrandom;

static TOKEN_LABEL: [CK_UTF8CHAR; 32usize] = *b"Kryoptic FIPS Token             ";
static MANUFACTURER_ID: [CK_UTF8CHAR; 32usize] = *b"Kryoptic                        ";
static TOKEN_MODEL: [CK_UTF8CHAR; 16usize] = *b"FIPS-140-3 v1   ";
static TOKEN_SERIAL: [CK_UTF8CHAR; 16usize] = *b"0000000000000000";

#[derive(Debug, Serialize, Deserialize)]
struct JsonToken {
    objects: Vec<JsonObject>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonObject {
    attributes: serde_json::Map<String, serde_json::Value>
}

#[derive(Debug, Clone)]
struct LoginData {
    pin: Option<Vec<u8>>,
    max_attempts: CK_ULONG,
    attempts: CK_ULONG,
    logged_in: bool,
}

impl LoginData {
    fn check_pin(&mut self, pin: &Vec<u8>) -> CK_RV {
        if self.attempts >= self.max_attempts {
            return CKR_PIN_LOCKED
        }
        match &self.pin {
            Some(p) => {
                if p == pin {
                    self.logged_in = true;
                    self.attempts = 0;
                    CKR_OK
                } else {
                    self.attempts += 1;
                    CKR_PIN_INCORRECT
                }
            },
            None => {
                CKR_USER_PIN_NOT_INITIALIZED
            }
        }
    }

    fn set_pin(&mut self, info: &CK_TOKEN_INFO, pin: &Vec<u8>) -> CK_RV {
        let pin_len = pin.len() as CK_ULONG;
        if info.ulMaxPinLen != CK_EFFECTIVELY_INFINITE {
            if pin_len > info.ulMaxPinLen {
                return CKR_PIN_LEN_RANGE
            }
        }
        if pin_len < info.ulMinPinLen {
            return CKR_PIN_LEN_RANGE
        }
        self.pin = Some(pin.clone());
        self.max_attempts = 10;
        self.attempts = 0;
        CKR_OK
    }

    fn change_pin(&mut self, info: &CK_TOKEN_INFO, pin: &Vec<u8>, old: &Vec<u8>) -> CK_RV {
        let ret = self.check_pin(old);
        if ret != CKR_OK {
            return ret
        }
        self.set_pin(info, pin)
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    info: CK_TOKEN_INFO,
    filename: String,
    objects: HashMap<String, Object>,
    dirty: bool,
    so_login: LoginData,
    user_login: LoginData,
    handles: HashMap<CK_OBJECT_HANDLE, String>,
    next_handle: CK_OBJECT_HANDLE,
}

impl Token {
    pub fn load(filename: String) -> KResult<Token> {

        let mut t = Token {
            info: CK_TOKEN_INFO {
                label: TOKEN_LABEL,
                manufacturerID: MANUFACTURER_ID,
                model: TOKEN_MODEL,
                serialNumber: TOKEN_SERIAL,
                flags: CKF_RNG | CKF_LOGIN_REQUIRED,
                ulMaxSessionCount: CK_EFFECTIVELY_INFINITE,
                ulSessionCount: 0,
                ulMaxRwSessionCount: CK_EFFECTIVELY_INFINITE,
                ulRwSessionCount: 0,
                ulMaxPinLen: CK_EFFECTIVELY_INFINITE,
                ulMinPinLen: 8,
                ulTotalPublicMemory: 0,
                ulFreePublicMemory: 0,
                ulTotalPrivateMemory: 0,
                ulFreePrivateMemory: 0,
                hardwareVersion: CK_VERSION {
                    major: 0,
                    minor: 0,
                },
                firmwareVersion: CK_VERSION {
                    major: 0,
                    minor: 0,
                },
                utcTime: *b"0000000000000000",
            },
            filename: filename,
            objects: HashMap::new(),
            so_login: LoginData {
                pin: None,
                max_attempts: 0,
                attempts: 0,
                logged_in: false,
            },
            user_login: LoginData {
                pin: None,
                max_attempts: 0,
                attempts: 0,
                logged_in: false,
            },
            dirty: false,
            handles: HashMap::new(),
            next_handle: 1,
        };

        match std::fs::File::open(&t.filename) {
            Ok(f) => match serde_json::from_reader::<std::fs::File, JsonToken>(f) {
                Ok(j) => t.json_to_objects(&j.objects)?,
                Err(e) => return Err(KError::JsonError(e)),
            },
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => return Ok(t),
                _ => return Err(KError::FileError(e)),
            }
        };
        t.info.flags |= CKF_TOKEN_INITIALIZED;
        Ok(t)
    }

    pub fn is_initialized(&self) -> bool {
        self.info.flags & CKF_TOKEN_INITIALIZED == CKF_TOKEN_INITIALIZED
    }

    fn store_pin_object(&mut self, uid: String, label: String,
                        pin: Vec<u8>) -> KResult<()> {
        match self.objects.get_mut(&uid) {
            Some(obj) => {
                obj.set_attr(attribute::from_bytes(CKA_VALUE, pin))?;
            },
            None => {
                let mut obj = Object::new(self.next_object_handle());
                obj.set_attr(attribute::from_string(CKA_UNIQUE_ID,
                                                    uid.clone()))?;
                obj.set_attr(attribute::from_bool(CKA_TOKEN, true))?;
                obj.set_attr(attribute::from_ulong(CKA_CLASS,
                                                   CKO_SECRET_KEY))?;
                obj.set_attr(attribute::from_ulong(CKA_KEY_TYPE,
                                                   CKK_GENERIC_SECRET))?;
                obj.set_attr(attribute::from_string(CKA_LABEL, label))?;
                obj.set_attr(attribute::from_bytes(CKA_VALUE, pin))?;
                self.handles.insert(obj.get_handle(), uid.clone());
                self.objects.insert(uid, obj);
            },
        }
        return Ok(())
    }

    pub fn initialize(&mut self, pin: &Vec<u8>, _label: &Vec<u8>) -> CK_RV {
        let ret = if self.is_initialized() {
            self.login(CKU_SO, pin)
        } else {
            self.so_login.set_pin(&self.info, pin)
        };
        if ret != CKR_OK {
            return ret;
        }
        self.so_login.logged_in = false;

        self.objects = HashMap::new();
        self.handles = HashMap::new();
        self.next_handle = 1;
        self.dirty = true;

        /* add pin to so_object */
        match self.store_pin_object("0".to_string(),
                                    "SO PIN".to_string(),
                                    pin.clone()) {
            Ok(()) => (),
            Err(_) => return CKR_GENERAL_ERROR,
        }

        match self.save() {
            Ok(_) => {
                self.info.flags |= CKF_TOKEN_INITIALIZED;
                CKR_OK
            },
            Err(_) => CKR_GENERAL_ERROR,
        }
    }

    fn next_object_handle(&mut self) -> CK_SESSION_HANDLE {
        /* if we ever implement reloading from file,
         * we'll want to pass the CKA_UNIQUE_ID object to this call and look
         * in the handles cache to see if a handle has already been assigned
         * to this object before */
        let handle = self.next_handle;
        self.next_handle += 1;
        handle
    }

    fn objects_to_json(&self) -> Vec<JsonObject> {
        let mut jobjs = Vec::new();

        for (_h, o) in &self.objects {
            match o.get_attr_as_bool(CKA_TOKEN) {
                Ok(t) => if !t {
                    continue;
                },
                Err(_) => continue,
            }
            let mut jo = JsonObject {
                attributes: serde_json::Map::new()
            };
            for a in o.get_attributes() {
                jo.attributes.insert(a.name(), a.json_value());
            }
            jobjs.push(jo);
        }
        jobjs
    }

    fn json_to_objects(&mut self, jobjs: &Vec<JsonObject>) -> KResult<()> {
        for jo in jobjs {
            let mut obj = Object::new(self.next_object_handle());
            let mut uid: String = String::new();
            for (key, val) in &jo.attributes {
                let attr = attribute::from_value(key.clone(), &val)?;
                obj.set_attr(attr)?;
                if key == "CKA_UNIQUE_ID" {
                    uid = match val.as_str() {
                        Some(s) => s.to_string(),
                        None => return err_rv!(CKR_DEVICE_ERROR),
                    }
                }
            }
            if uid.len() == 0 {
                return err_rv!(CKR_DEVICE_ERROR);
            }
            self.handles.insert(obj.get_handle(), uid.clone());
            self.objects.insert(uid, obj);
        }
        Ok(())
    }

    pub fn get_object_by_handle(&self, handle: CK_OBJECT_HANDLE, checks: bool) -> KResult<&Object> {
        let obj = match self.handles.get(&handle) {
            Some(s) => match self.objects.get(s) {
                Some(o) => o,
                None => return err_not_found!{s.clone()},
            },
            None => return err_rv!(CKR_OBJECT_HANDLE_INVALID),
        };
        if checks && !self.user_login.logged_in && obj.is_private() {
            return err_rv!(CKR_OBJECT_HANDLE_INVALID)
        }
        Ok(obj)
    }

    fn validate_pin_obj(&self, obj: &Object, label: String) -> KResult<(Vec<u8>, CK_ULONG)> {
        if obj.get_attr_as_ulong(CKA_CLASS)? != CKO_SECRET_KEY {
            return err_rv!(CKR_GENERAL_ERROR);
        }
        if obj.get_attr_as_ulong(CKA_KEY_TYPE)? != CKK_GENERIC_SECRET {
            return err_rv!(CKR_GENERAL_ERROR);
        }
        if obj.get_attr_as_string(CKA_LABEL)? != label {
            return err_rv!(CKR_GENERAL_ERROR);
        }
        let value = obj.get_attr_as_bytes(CKA_VALUE)?;
        let max = match obj.get_attr_as_ulong(KRYATTR_MAX_LOGIN_ATTEMPTS) {
            Ok(n) => n,
            Err(_) => 10,
        };

        Ok((value.clone(), max as CK_ULONG))
    }

    fn get_so_login_data(&mut self) -> KResult<()> {
        if self.so_login.pin.is_none() {
            let obj = match self.objects.get(&"0".to_string()) {
                Some(o) => o,
                None => return err_rv!(CKR_GENERAL_ERROR),
            };
            let (pin, max) = self.validate_pin_obj(obj, "SO PIN".to_string())?;
            self.so_login.pin = Some(pin);
            self.so_login.max_attempts = max;
        }
        Ok(())
    }

    fn get_user_login_data(&mut self) -> KResult<()> {
        if self.user_login.pin.is_none() {
            let obj = match self.objects.get(&"1".to_string()) {
                Some(o) => o,
                None => return err_rv!(CKR_USER_PIN_NOT_INITIALIZED),
            };
            let (pin, max) = self.validate_pin_obj(obj, "User PIN".to_string())?;
            self.user_login.pin = Some(pin);
            self.user_login.max_attempts = max;
        }
        Ok(())
    }

    pub fn login(&mut self, user_type: CK_USER_TYPE, pin: &Vec<u8>) -> CK_RV {
        match user_type {
            CKU_SO => {
                if self.so_login.logged_in {
                    return CKR_USER_ALREADY_LOGGED_IN
                }
                if self.user_login.logged_in {
                    return CKR_USER_ANOTHER_ALREADY_LOGGED_IN;
                }
                match self.get_so_login_data() {
                    Ok(_) => (),
                    Err(e) => match e {
                        KError::RvError(e) => return e.rv,
                        _ => return CKR_GENERAL_ERROR,
                    },
                }
                self.so_login.check_pin(pin)
            },
            CKU_USER => {
                if self.user_login.logged_in {
                    return CKR_USER_ALREADY_LOGGED_IN
                }
                if self.so_login.logged_in {
                    return CKR_USER_ANOTHER_ALREADY_LOGGED_IN;
                }
                match self.get_user_login_data() {
                    Ok(_) => (),
                    Err(e) => match e {
                        KError::RvError(e) => return e.rv,
                        _ => return CKR_GENERAL_ERROR,
                    },
                }
                self.user_login.check_pin(pin)
            },
            _ => return CKR_USER_TYPE_INVALID,
        }
    }

    pub fn logout(&mut self) -> CK_RV {
        if self.user_login.logged_in {
            self.user_login.logged_in = false;
            return CKR_OK
        }
        if self.so_login.logged_in {
            self.so_login.logged_in = false;
            return CKR_OK
        }
        CKR_USER_NOT_LOGGED_IN
    }

    pub fn is_logged_in(&self, user_type: CK_USER_TYPE) -> bool {
        match user_type {
            CK_UNAVAILABLE_INFORMATION => {
                self.so_login.logged_in || self.user_login.logged_in
            },
            CKU_SO => self.so_login.logged_in,
            CKU_USER => self.user_login.logged_in,
            _ => false,
        }
    }

    pub fn set_pin(&mut self, user_type: CK_USER_TYPE, pin: &Vec<u8>, old: Option<&Vec<u8>>) -> CK_RV {
        let utype = match user_type {
            CK_UNAVAILABLE_INFORMATION => {
                if self.so_login.logged_in {
                    CKU_SO
                } else {
                    CKU_USER
                }
            },
            CKU_USER => CKU_USER,
            CKU_SO => CKU_SO,
            _ => return CKR_GENERAL_ERROR,
        };

        match utype {
            CKU_USER => {
                let ret = if self.so_login.logged_in {
                    self.user_login.set_pin(&self.info, pin)
                } else {
                    if old.is_none() {
                        return CKR_PIN_INCORRECT
                    }
                    self.user_login.change_pin(&self.info, pin, old.unwrap())
                };
                if ret != CKR_OK {
                    return ret
                }
                /* update pin in storage */
                match self.store_pin_object("1".to_string(),
                                            "User PIN".to_string(),
                                            pin.clone()) {
                    Ok(()) => (),
                    Err(_) => return CKR_GENERAL_ERROR,
                }
            },
            CKU_SO => {
                if old.is_none() {
                    return CKR_PIN_INCORRECT
                }
                let ret = self.so_login.change_pin(&self.info, pin,
                                                   old.unwrap());
                if ret != CKR_OK {
                    return ret
                }
                /* update pin in storage */
                match self.store_pin_object("0".to_string(),
                                            "SO PIN".to_string(),
                                            pin.clone()) {
                    Ok(()) => (),
                    Err(_) => return CKR_GENERAL_ERROR,
                }
            },
            _ => return CKR_GENERAL_ERROR,
        }

        self.dirty = true;
        match self.save() {
            Ok(()) => CKR_OK,
            Err(_) => CKR_GENERAL_ERROR,
        }
    }

    pub fn save(&self) -> KResult<()> {
        if !self.dirty {
            return Ok(())
        }
        let token = JsonToken {
            objects: self.objects_to_json(),
        };
        let j = match serde_json::to_string_pretty(&token) {
            Ok(j) => j,
            Err(e) => return Err(KError::JsonError(e)),
        };
        match std::fs::write(&self.filename, j) {
            Ok(_) => Ok(()),
            Err(e) => Err(KError::FileError(e)),
        }
    }

    pub fn create_object(&mut self, session: &mut Session, template: &[CK_ATTRIBUTE]) -> KResult<CK_OBJECT_HANDLE> {

        if !self.user_login.logged_in {
            return err_rv!(CKR_USER_NOT_LOGGED_IN);
        }

        let obj = object::create(self.next_object_handle(), template)?;
        let handle = obj.get_handle();
        match obj.get_attr_as_bool(CKA_TOKEN) {
            Ok(t) => if t {
                if !session.is_writable() {
                    return err_rv!(CKR_SESSION_READ_ONLY);
                }
                self.dirty = true;
            } else {
                session.add_handle(handle);
            },
            Err(_) => return err_rv!(CKR_GENERAL_ERROR),
        }
        let uid = obj.get_attr_as_string(CKA_UNIQUE_ID)?;
        self.handles.insert(handle, uid.clone());
        self.objects.insert(uid.clone(), obj);
        Ok(handle)
    }

    pub fn get_token_info(&self) -> &CK_TOKEN_INFO {
        &self.info
    }

    pub fn search(&self, session: &mut Session, template: &[CK_ATTRIBUTE]) -> KResult<()> {
        session.reset_search_handles();

        for (_, o) in &self.objects {
            if !self.user_login.logged_in && o.is_private() {
                continue;
            }

            if o.match_template(template) {
                session.add_search_handle(o.get_handle());
            }
        }
        Ok(())
    }

    pub fn get_object_attrs(&self, handle: CK_OBJECT_HANDLE, template: &mut [CK_ATTRIBUTE]) -> KResult<()> {
        match self.get_object_by_handle(handle, true) {
            Ok(o) => o.fill_template(template),
            Err(e) => return Err(e),
        }
    }

    pub fn generate_random(&self, buffer: &mut [u8]) -> KResult<()> {
        /* NOTE: this is just a placeholder to get somethjing going */
        if buffer.len() > 256 {
            return err_rv!(CKR_ARGUMENTS_BAD);
        }
        if getrandom::getrandom(buffer).is_err() {
            return err_rv!(CKR_GENERAL_ERROR)
        }
        Ok(())
    }
}
