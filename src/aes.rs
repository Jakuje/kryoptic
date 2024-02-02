// Copyright 2024 Simo Sorce
// See LICENSE.txt file for terms

use super::attribute;
use super::error;
use super::interface;
use super::object;
use super::{attr_element, err_rv};

use attribute::{from_bool, from_bytes};
use error::{KError, KResult};
use interface::*;
use object::{
    CommonKeyTemplate, OAFlags, Object, ObjectAttr, ObjectTemplate,
    ObjectTemplates, ObjectType, SecretKeyTemplate,
};

use super::mechanism;
use mechanism::*;

use once_cell::sync::Lazy;
use std::fmt::Debug;

fn check_key_len(len: usize) -> KResult<()> {
    match len {
        16 | 24 | 32 => Ok(()),
        _ => err_rv!(CKR_KEY_SIZE_RANGE),
    }
}

fn check_key_object(key: &Object, op: CK_ULONG) -> KResult<()> {
    match key.get_attr_as_ulong(CKA_CLASS)? {
        CKO_SECRET_KEY => match key.get_attr_as_ulong(CKA_KEY_TYPE)? {
            CKK_AES => (),
            CKK_GENERIC_SECRET => (),
            _ => return err_rv!(CKR_KEY_TYPE_INCONSISTENT),
        },
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

#[derive(Debug)]
pub struct AesKeyTemplate {
    attributes: Vec<ObjectAttr>,
}

impl AesKeyTemplate {
    fn new() -> AesKeyTemplate {
        let mut data: AesKeyTemplate = AesKeyTemplate {
            attributes: Vec::new(),
        };
        data.attributes.append(&mut data.init_common_object_attrs());
        data.attributes
            .append(&mut data.init_common_storage_attrs());
        data.attributes.append(&mut data.init_common_key_attrs());
        data.attributes
            .append(&mut data.init_common_secret_key_attrs());
        data.attributes.push(attr_element!(CKA_VALUE; OAFlags::Defval | OAFlags::Sensitive | OAFlags::RequiredOnCreate | OAFlags::UnsettableOnGenerate | OAFlags::UnsettableOnUnwrap; from_bytes; val Vec::new()));
        data.attributes.push(attr_element!(CKA_VALUE_LEN; OAFlags::RequiredOnGenerate | OAFlags::UnsettableOnCreate; from_bytes; val Vec::new()));

        /* default to private */
        let private = attr_element!(CKA_PRIVATE; OAFlags::Defval | OAFlags::ChangeOnCopy; from_bool; val true);
        match data
            .attributes
            .iter()
            .position(|x| x.get_type() == CKA_PRIVATE)
        {
            Some(idx) => data.attributes[idx] = private,
            None => data.attributes.push(private),
        }

        data
    }
}

impl ObjectTemplate for AesKeyTemplate {
    fn create(&self, template: &[CK_ATTRIBUTE]) -> KResult<Object> {
        let obj = self.default_object_create(template)?;

        let val = obj.get_attr_as_bytes(CKA_VALUE)?;
        check_key_len(val.len())?;

        Ok(obj)
    }

    fn get_attributes(&self) -> &Vec<ObjectAttr> {
        &self.attributes
    }
}

impl CommonKeyTemplate for AesKeyTemplate {
    fn get_attributes(&self) -> &Vec<ObjectAttr> {
        &self.attributes
    }
}

impl SecretKeyTemplate for AesKeyTemplate {
    fn get_attributes(&self) -> &Vec<ObjectAttr> {
        &self.attributes
    }

    fn default_object_unwrap(
        &self,
        template: &[CK_ATTRIBUTE],
    ) -> KResult<Object> {
        ObjectTemplate::default_object_unwrap(self, template)
    }
}

static AES_KEY_TEMPLATE: Lazy<Box<dyn ObjectTemplate>> =
    Lazy::new(|| Box::new(AesKeyTemplate::new()));

#[derive(Debug)]
struct AesMechanism {
    info: CK_MECHANISM_INFO,
}

impl Mechanism for AesMechanism {
    fn info(&self) -> &CK_MECHANISM_INFO {
        &self.info
    }

    fn encryption_new(
        &self,
        mech: &CK_MECHANISM,
        key: &Object,
    ) -> KResult<Box<dyn Encryption>> {
        if self.info.flags & CKF_ENCRYPT != CKF_ENCRYPT {
            return err_rv!(CKR_MECHANISM_INVALID);
        }
        match check_key_object(key, CKA_ENCRYPT) {
            Ok(_) => (),
            Err(e) => return Err(e),
        }
        Ok(Box::new(AesOperation::encrypt_new(mech, key)?))
    }

    fn decryption_new(
        &self,
        mech: &CK_MECHANISM,
        key: &Object,
    ) -> KResult<Box<dyn Decryption>> {
        if self.info.flags & CKF_DECRYPT != CKF_DECRYPT {
            return err_rv!(CKR_MECHANISM_INVALID);
        }
        match check_key_object(key, CKA_DECRYPT) {
            Ok(_) => (),
            Err(e) => return Err(e),
        }
        Ok(Box::new(AesOperation::decrypt_new(mech, key)?))
    }

    fn generate_key(
        &self,
        mech: &CK_MECHANISM,
        template: &[CK_ATTRIBUTE],
    ) -> KResult<Object> {
        if mech.mechanism != CKM_AES_KEY_GEN {
            return err_rv!(CKR_MECHANISM_INVALID);
        }
        let mut key = AES_KEY_TEMPLATE.default_object_generate(template)?;
        if !key.check_or_set_attr(attribute::from_ulong(
            CKA_CLASS,
            CKO_SECRET_KEY,
        ))? {
            return err_rv!(CKR_TEMPLATE_INCONSISTENT);
        }
        if !key
            .check_or_set_attr(attribute::from_ulong(CKA_KEY_TYPE, CKK_AES))?
        {
            return err_rv!(CKR_TEMPLATE_INCONSISTENT);
        }

        let value_len = key.get_attr_as_ulong(CKA_VALUE_LEN)? as usize;
        check_key_len(value_len)?;
        key.del_attr(CKA_VALUE_LEN);

        let mut value: Vec<u8> = vec![0; value_len];
        match super::CSPRNG
            .with(|rng| rng.borrow_mut().generate_random(value.as_mut_slice()))
        {
            Ok(()) => (),
            Err(e) => return Err(e),
        }
        key.set_attr(attribute::from_bytes(CKA_VALUE, value))?;

        Ok(key)
    }

    fn wrap_key(
        &self,
        mech: &CK_MECHANISM,
        wrapping_key: &Object,
        key: &Object,
        data: CK_BYTE_PTR,
        data_len: CK_ULONG_PTR,
        key_template: &Box<dyn ObjectTemplate>,
    ) -> KResult<()> {
        if self.info.flags & CKF_WRAP != CKF_WRAP {
            return err_rv!(CKR_MECHANISM_INVALID);
        }

        AesOperation::wrap(
            mech,
            wrapping_key,
            key_template.export_for_wrapping(key)?,
            data,
            data_len,
        )
    }

    fn unwrap_key(
        &self,
        mech: &CK_MECHANISM,
        wrapping_key: &Object,
        data: &[u8],
        template: &[CK_ATTRIBUTE],
        key_template: &Box<dyn ObjectTemplate>,
    ) -> KResult<Object> {
        if self.info.flags & CKF_UNWRAP != CKF_UNWRAP {
            return err_rv!(CKR_MECHANISM_INVALID);
        }
        let keydata = AesOperation::unwrap(mech, wrapping_key, data)?;
        key_template.import_from_wrapped(keydata, template)
    }
}

pub fn register(mechs: &mut Mechanisms, ot: &mut ObjectTemplates) {
    mechs.add_mechanism(
        CKM_AES_ECB,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_ENCRYPT | CKF_DECRYPT | CKF_WRAP | CKF_UNWRAP,
            },
        }),
    );

    mechs.add_mechanism(
        CKM_AES_CBC,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_ENCRYPT | CKF_DECRYPT | CKF_WRAP | CKF_UNWRAP,
            },
        }),
    );

    mechs.add_mechanism(
        CKM_AES_CBC_PAD,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_ENCRYPT | CKF_DECRYPT | CKF_WRAP | CKF_UNWRAP,
            },
        }),
    );

    #[cfg(not(feature = "fips"))]
    mechs.add_mechanism(
        CKM_AES_OFB,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_ENCRYPT | CKF_DECRYPT,
            },
        }),
    );

    #[cfg(not(feature = "fips"))]
    mechs.add_mechanism(
        CKM_AES_CFB128,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_ENCRYPT | CKF_DECRYPT,
            },
        }),
    );

    #[cfg(not(feature = "fips"))]
    mechs.add_mechanism(
        CKM_AES_CFB1,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_ENCRYPT | CKF_DECRYPT,
            },
        }),
    );

    #[cfg(not(feature = "fips"))]
    mechs.add_mechanism(
        CKM_AES_CFB8,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_ENCRYPT | CKF_DECRYPT,
            },
        }),
    );
    /* OpenSSL does not implement AES CFB-64 */

    mechs.add_mechanism(
        CKM_AES_CTR,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_ENCRYPT | CKF_DECRYPT | CKF_WRAP | CKF_UNWRAP,
            },
        }),
    );

    mechs.add_mechanism(
        CKM_AES_CTS,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_ENCRYPT | CKF_DECRYPT | CKF_WRAP | CKF_UNWRAP,
            },
        }),
    );

    mechs.add_mechanism(
        CKM_AES_GCM,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_ENCRYPT | CKF_DECRYPT | CKF_WRAP | CKF_UNWRAP,
            },
        }),
    );

    mechs.add_mechanism(
        CKM_AES_KEY_GEN,
        Box::new(AesMechanism {
            info: CK_MECHANISM_INFO {
                ulMinKeySize: 16,
                ulMaxKeySize: 32,
                flags: CKF_GENERATE,
            },
        }),
    );

    ot.add_template(
        ObjectType::new(CKO_SECRET_KEY, CKK_AES),
        &AES_KEY_TEMPLATE,
    );
}

#[cfg(feature = "fips")]
include!("ossl/aes.rs");

#[cfg(not(feature = "fips"))]
include!("ossl/aes.rs");
