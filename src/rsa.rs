// Copyright 2023 Simo Sorce
// See LICENSE.txt file for terms

use super::attribute;
use super::cryptography;
use super::error;
use super::interface;
use super::mechanism;
use super::object;
use super::token;
use super::{attr_element, bytes_attr_not_empty, err_rv};
use attribute::{from_bytes, from_ulong};
use cryptography::*;
use error::{KError, KResult};
use interface::*;
use mechanism::*;
use object::{
    CommonKeyTemplate, Object, ObjectAttr, ObjectTemplate, ObjectTemplates,
    ObjectType, PrivKeyTemplate, PubKeyTemplate,
};
use std::fmt::Debug;
use token::RNG;

pub const MIN_RSA_SIZE_BITS: usize = 1024;
pub const MIN_RSA_SIZE_BYTES: usize = MIN_RSA_SIZE_BITS / 8;

#[derive(Debug)]
pub struct RSAPubTemplate {
    template: Vec<ObjectAttr>,
}

impl RSAPubTemplate {
    pub fn new() -> RSAPubTemplate {
        let mut data: RSAPubTemplate = RSAPubTemplate {
            template: Vec::new(),
        };
        data.init_common_object_attrs();
        data.init_common_storage_attrs();
        data.init_common_key_attrs();
        data.init_common_public_key_attrs();
        data.template.push(attr_element!(CKA_MODULUS; req true; def false; from_bytes; val Vec::new()));
        data.template.push(attr_element!(CKA_MODULUS_BITS; req false; def false; from_ulong; val 0));
        data.template.push(attr_element!(CKA_PUBLIC_EXPONENT; req true; def false; from_bytes; val Vec::new()));
        data
    }
}

impl ObjectTemplate for RSAPubTemplate {
    fn create(&self, mut obj: Object) -> KResult<Object> {
        let mut attr_checker = self.template.clone();

        let mut ret =
            self.basic_object_attrs_checks(&mut obj, &mut attr_checker);
        if ret != CKR_OK {
            return err_rv!(ret);
        }

        ret = self.pubkey_create_attrs_checks(&mut obj, &mut attr_checker);
        if ret != CKR_OK {
            return err_rv!(ret);
        }

        let modulus = match obj.get_attr_as_bytes(CKA_MODULUS) {
            Ok(m) => m,
            Err(_) => return err_rv!(CKR_TEMPLATE_INCOMPLETE),
        };
        match obj.get_attr_as_ulong(CKA_MODULUS_BITS) {
            Ok(_) => return err_rv!(CKR_ATTRIBUTE_VALUE_INVALID),
            Err(e) => match e {
                KError::NotFound(_) => (),
                _ => return Err(e),
            },
        }
        if modulus.len() < MIN_RSA_SIZE_BYTES {
            return err_rv!(CKR_ATTRIBUTE_VALUE_INVALID);
        }
        bytes_attr_not_empty!(obj; CKA_PUBLIC_EXPONENT);

        Ok(obj)
    }

    fn get_template(&mut self) -> &mut Vec<ObjectAttr> {
        &mut self.template
    }
}

impl CommonKeyTemplate for RSAPubTemplate {
    fn get_template(&mut self) -> &mut Vec<ObjectAttr> {
        &mut self.template
    }
}

impl PubKeyTemplate for RSAPubTemplate {
    fn get_template(&mut self) -> &mut Vec<ObjectAttr> {
        &mut self.template
    }
}

#[derive(Debug)]
pub struct RSAPrivTemplate {
    template: Vec<ObjectAttr>,
}

impl RSAPrivTemplate {
    pub fn new() -> RSAPrivTemplate {
        let mut data: RSAPrivTemplate = RSAPrivTemplate {
            template: Vec::new(),
        };
        data.init_common_object_attrs();
        data.init_common_storage_attrs();
        data.init_common_key_attrs();
        data.init_common_private_key_attrs();
        data.template.push(attr_element!(CKA_MODULUS; req true; def false; from_bytes; val Vec::new()));
        data.template.push(attr_element!(CKA_PUBLIC_EXPONENT; req true; def false; from_bytes; val Vec::new()));
        data.template.push(attr_element!(CKA_PRIVATE_EXPONENT; req true; def false; from_bytes; val Vec::new()));
        data.template.push(attr_element!(CKA_PRIME_1; req false; def false; from_bytes; val Vec::new()));
        data.template.push(attr_element!(CKA_PRIME_2; req false; def false; from_bytes; val Vec::new()));
        data.template.push(attr_element!(CKA_EXPONENT_1; req false; def false; from_bytes; val Vec::new()));
        data.template.push(attr_element!(CKA_EXPONENT_2; req false; def false; from_bytes; val Vec::new()));
        data.template.push(attr_element!(CKA_COEFFICIENT; req false; def false; from_bytes; val Vec::new()));
        data
    }
}

impl ObjectTemplate for RSAPrivTemplate {
    fn create(&self, mut obj: Object) -> KResult<Object> {
        let mut attr_checker = self.template.clone();

        let mut ret =
            self.basic_object_attrs_checks(&mut obj, &mut attr_checker);
        if ret != CKR_OK {
            return err_rv!(ret);
        }

        ret = self.privkey_create_attrs_checks(&mut obj, &mut attr_checker);
        if ret != CKR_OK {
            return err_rv!(ret);
        }

        let modulus = match obj.get_attr_as_bytes(CKA_MODULUS) {
            Ok(m) => m,
            Err(_) => return err_rv!(CKR_TEMPLATE_INCOMPLETE),
        };
        if modulus.len() < MIN_RSA_SIZE_BYTES {
            return err_rv!(CKR_ATTRIBUTE_VALUE_INVALID);
        }
        bytes_attr_not_empty!(obj; CKA_PUBLIC_EXPONENT);
        bytes_attr_not_empty!(obj; CKA_PRIVATE_EXPONENT);

        Ok(obj)
    }

    fn get_template(&mut self) -> &mut Vec<ObjectAttr> {
        &mut self.template
    }
}

impl CommonKeyTemplate for RSAPrivTemplate {
    fn get_template(&mut self) -> &mut Vec<ObjectAttr> {
        &mut self.template
    }
}

impl PrivKeyTemplate for RSAPrivTemplate {
    fn get_template(&mut self) -> &mut Vec<ObjectAttr> {
        &mut self.template
    }
}

fn check_key_object(key: &Object, public: bool, op: CK_ULONG) -> KResult<()> {
    match key.get_attr_as_ulong(CKA_CLASS)? {
        CKO_PUBLIC_KEY => {
            if !public {
                return err_rv!(CKR_KEY_TYPE_INCONSISTENT);
            }
        }
        CKO_PRIVATE_KEY => {
            if public {
                return err_rv!(CKR_KEY_TYPE_INCONSISTENT);
            }
        }
        _ => return err_rv!(CKR_KEY_TYPE_INCONSISTENT),
    }
    match key.get_attr_as_ulong(CKA_KEY_TYPE)? {
        CKK_RSA => (),
        _ => return err_rv!(CKR_KEY_TYPE_INCONSISTENT),
    }
    match key.get_attr_as_bool(op) {
        Ok(avail) => {
            if !avail {
                return err_rv!(CKR_KEY_FUNCTION_NOT_PERMITTED);
            }
        }
        Err(_) => return err_rv!(CKR_KEY_FUNCTION_NOT_PERMITTED),
    }
    Ok(())
}

macro_rules! import_mpz {
    ($obj:expr; $id:expr; $mpz:expr) => {{
        let x = match $obj.get_attr_as_bytes($id) {
            Ok(b) => b,
            Err(_) => return err_rv!(CKR_DEVICE_ERROR),
        };
        unsafe {
            __gmpz_import(
                &mut $mpz,
                x.len(),
                1,
                1,
                0,
                0,
                x.as_ptr() as *const ::std::os::raw::c_void,
            );
        }
    }};
}

fn object_to_rsa_public_key(key: &Object) -> KResult<rsa_public_key> {
    let mut k: rsa_public_key = rsa_public_key::default();
    unsafe {
        nettle_rsa_public_key_init(&mut k);
    }
    import_mpz!(key; CKA_PUBLIC_EXPONENT; k.e[0]);
    import_mpz!(key; CKA_MODULUS; k.n[0]);
    if unsafe { nettle_rsa_public_key_prepare(&mut k) } == 0 {
        err_rv!(CKR_GENERAL_ERROR)
    } else {
        Ok(k)
    }
}

fn object_to_rsa_private_key(key: &Object) -> KResult<rsa_private_key> {
    let mut k: rsa_private_key = rsa_private_key::default();
    unsafe {
        nettle_rsa_private_key_init(&mut k);
    }
    import_mpz!(key; CKA_PRIVATE_EXPONENT; k.d[0]);
    import_mpz!(key; CKA_PRIME_1; k.p[0]);
    import_mpz!(key; CKA_PRIME_2; k.q[0]);
    import_mpz!(key; CKA_EXPONENT_1; k.a[0]);
    import_mpz!(key; CKA_EXPONENT_2; k.b[0]);
    import_mpz!(key; CKA_COEFFICIENT; k.c[0]);
    if unsafe { nettle_rsa_private_key_prepare(&mut k) } == 0 {
        err_rv!(CKR_GENERAL_ERROR)
    } else {
        Ok(k)
    }
}

#[derive(Debug)]
struct RsaPKCSMechanism {
    info: CK_MECHANISM_INFO,
}

impl Mechanism for RsaPKCSMechanism {
    fn info(&self) -> &CK_MECHANISM_INFO {
        &self.info
    }

    fn encryption_new(
        &self,
        mech: &CK_MECHANISM,
        key: &Object,
    ) -> KResult<Box<dyn Operation>> {
        if mech.mechanism != CKM_RSA_PKCS {
            return err_rv!(CKR_MECHANISM_INVALID);
        }
        if self.info.flags & CKF_ENCRYPT != CKF_ENCRYPT {
            return err_rv!(CKR_MECHANISM_INVALID);
        }
        match check_key_object(key, true, CKA_ENCRYPT) {
            Ok(_) => (),
            Err(e) => return Err(e),
        }
        let op = RsaPKCSOperation {
            mech: mech.mechanism,
            public_key: Some(object_to_rsa_public_key(key)?),
            private_key: None,
            used: false,
            finalized: false,
        };
        Ok(Box::new(op))
    }

    fn decryption_new(
        &self,
        mech: &CK_MECHANISM,
        key: &Object,
    ) -> KResult<Box<dyn Operation>> {
        if mech.mechanism != CKM_RSA_PKCS {
            return err_rv!(CKR_MECHANISM_INVALID);
        }
        if self.info.flags & CKF_DECRYPT != CKF_DECRYPT {
            return err_rv!(CKR_MECHANISM_INVALID);
        }
        match check_key_object(key, false, CKA_DECRYPT) {
            Ok(_) => (),
            Err(e) => return Err(e),
        }
        let op = RsaPKCSOperation {
            mech: mech.mechanism,
            public_key: None,
            private_key: Some(object_to_rsa_private_key(key)?),
            used: false,
            finalized: false,
        };
        Ok(Box::new(op))
    }
}

pub fn register(mechs: &mut Mechanisms, ot: &mut ObjectTemplates) {
    mechs.add_mechanism(
        CKM_RSA_PKCS,
        Box::new(RsaPKCSMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 1024,
                ulMaxKeySize: 4096,
                flags: CKF_ENCRYPT | CKF_DECRYPT,
            },
        }),
    );

    ot.add_template(ObjectType::RSAPubKey, Box::new(RSAPubTemplate::new()));
    ot.add_template(ObjectType::RSAPrivKey, Box::new(RSAPrivTemplate::new()));
}

#[derive(Debug)]
struct RsaPKCSOperation {
    mech: CK_MECHANISM_TYPE,
    public_key: Option<rsa_public_key>,
    private_key: Option<rsa_private_key>,
    used: bool,
    finalized: bool,
}

impl BaseOperation for RsaPKCSOperation {
    fn mechanism(&self) -> CK_MECHANISM_TYPE {
        self.mech
    }
    fn used(&self) -> bool {
        self.used
    }
}

impl Encryption for RsaPKCSOperation {
    fn encrypt(
        &mut self,
        rng: &mut RNG,
        plain: &[u8],
        cipher: &mut [u8],
        inplace: bool,
    ) -> KResult<()> {
        self.used = true;

        let mut x: __mpz_struct = __mpz_struct::default();
        let key: *const rsa_public_key = match self.public_key {
            None => return err_rv!(CKR_GENERAL_ERROR),
            Some(ref k) => k,
        };

        if unsafe {
            __gmpz_init(&mut x);
            if inplace {
                nettle_rsa_encrypt(
                    key,
                    rng as *mut _ as *mut ::std::os::raw::c_void,
                    Some(get_random),
                    cipher.len(),
                    cipher.as_ptr(),
                    &mut x,
                )
            } else {
                nettle_rsa_encrypt(
                    key,
                    rng as *mut _ as *mut ::std::os::raw::c_void,
                    Some(get_random),
                    plain.len(),
                    plain.as_ptr(),
                    &mut x,
                )
            }
        } == 0
        {
            return err_rv!(CKR_GENERAL_ERROR);
        }

        cipher.fill(0);
        unsafe {
            let len = nettle_mpz_sizeinbase_256_u(&mut x);
            if len > cipher.len() {
                return err_rv!(CKR_BUFFER_TOO_SMALL);
            }
            let mut count: usize = 0;
            __gmpz_export(
                cipher.as_ptr() as *mut ::std::os::raw::c_void,
                &mut count,
                -1,
                1,
                0,
                0,
                &mut x,
            );
        }
        Ok(())
    }
    fn encrypt_update(
        &mut self,
        _rng: &mut RNG,
        plain: &[u8],
        cipher: &mut [u8],
        inplace: bool,
    ) -> KResult<()> {
        self.used = true;
        err_rv!(CKR_GENERAL_ERROR)
    }
    fn encrypt_final(
        &mut self,
        _rng: &mut RNG,
        cipher: &mut [u8],
    ) -> KResult<()> {
        self.finalized = true;
        Ok(())
    }
}

impl Decryption for RsaPKCSOperation {}

impl Operation for RsaPKCSOperation {}

unsafe extern "C" fn get_random(
    ctx: *mut ::std::os::raw::c_void,
    length: usize,
    dst: *mut u8,
) {
    let rng = unsafe { &mut *(ctx as *mut RNG) };
    let buf = unsafe { std::slice::from_raw_parts_mut(dst, length) };
    rng.generate_random(buf).unwrap();
}
