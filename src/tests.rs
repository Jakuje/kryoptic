// Copyright 2023 Simo Sorce
// See LICENSE.txt file for terms

use super::*;
use hex;
use std::ffi::CString;
use std::sync::Once;

const CK_ULONG_SIZE: usize = std::mem::size_of::<CK_ULONG>();
const CK_BBOOL_SIZE: usize = std::mem::size_of::<CK_BBOOL>();
macro_rules! make_attribute {
    ($type:expr, $value:expr, $length:expr) => {
        CK_ATTRIBUTE {
            type_: $type,
            pValue: $value as CK_VOID_PTR,
            ulValueLen: $length as CK_ULONG,
        }
    };
}

/* note that the following concoction to sync threads is not entirely race free
 * as it assumes all tests initialize before all of the others complete. */
static FINI: RwLock<u64> = RwLock::new(0);
static SYNC: RwLock<u64> = RwLock::new(0);

static INIT: Once = Once::new();
fn test_finalizer() -> Option<RwLockWriteGuard<'static, u64>> {
    let mut winner: Option<RwLockWriteGuard<u64>> = None;
    INIT.call_once(|| {
        winner = Some(FINI.write().unwrap());
    });
    winner
}

struct Slots {
    id: u64,
}

static SLOTS: RwLock<Slots> = RwLock::new(Slots { id: 0 });

struct TestData<'a> {
    slot: CK_SLOT_ID,
    filename: &'a str,
    created: bool,
    finalize: Option<RwLockWriteGuard<'a, u64>>,
    sync: Option<RwLockReadGuard<'a, u64>>,
}

impl TestData<'_> {
    fn new<'a>(filename: &'a str) -> TestData {
        let mut slots = SLOTS.write().unwrap();
        slots.id += 1;
        TestData {
            slot: slots.id,
            filename: filename,
            created: false,
            finalize: test_finalizer(),
            sync: Some(SYNC.read().unwrap()),
        }
    }

    fn setup_db(&mut self) {
        let test_token = serde_json::json!({
            "objects": [{
                "attributes": {
                    "CKA_UNIQUE_ID": "0",
                    "CKA_CLASS": 4,
                    "CKA_KEY_TYPE": 16,
                    "CKA_LABEL": "SO PIN",
                    "CKA_VALUE": "MTIzNDU2Nzg=",
                    "CKA_TOKEN": true
                }
            }, {
                "attributes": {
                    "CKA_UNIQUE_ID": "1",
                    "CKA_CLASS": 4,
                    "CKA_KEY_TYPE": 16,
                    "CKA_LABEL": "User PIN",
                    "CKA_VALUE": "MTIzNDU2Nzg=",
                    "CKA_TOKEN": true
                }
            }, {
                "attributes": {
                    "CKA_UNIQUE_ID": "2",
                    "CKA_CLASS": 2,
                    "CKA_KEY_TYPE": 0,
                    "CKA_DESTROYABLE": false,
                    "CKA_ID": "AQ==",
                    "CKA_LABEL": "Test RSA Key",
                    "CKA_MODIFIABLE": true,
                    "CKA_MODULUS": "AQIDBAUGBwg=",
                    "CKA_PRIVATE": false,
                    "CKA_PUBLIC_EXPONENT": "AQAB",
                    "CKA_TOKEN": true
                }
            }, {
                "attributes": {
                    "CKA_UNIQUE_ID": "3",
                    "CKA_CLASS": 3,
                    "CKA_KEY_TYPE": 0,
                    "CKA_DESTROYABLE": false,
                    "CKA_ID": "AQ==",
                    "CKA_LABEL": "Test RSA Key",
                    "CKA_MODIFIABLE": false,
                    "CKA_MODULUS": "AQIDBAUGBwg=",
                    "CKA_PRIVATE": true,
                    "CKA_SENSITIVE": true,
                    "CKA_EXTRACTABLE": false,
                    "CKA_PUBLIC_EXPONENT": "AQAB",
                    "CKA_PRIVATE_EXPONENT": "AQAD",
                    "CKA_TOKEN": true
                }
            }]
        });
        let file = std::fs::File::create(self.filename).unwrap();
        serde_json::to_writer_pretty(file, &test_token).unwrap();
        self.created = true;
    }

    fn get_slot(&self) -> CK_SLOT_ID {
        self.slot
    }

    fn mark_file_created(&mut self) {
        self.created = true;
    }

    fn make_init_args(&self) -> CK_C_INITIALIZE_ARGS {
        let reserved: String = format!("{}:{}", self.filename, self.slot);

        CK_C_INITIALIZE_ARGS {
            CreateMutex: None,
            DestroyMutex: None,
            LockMutex: None,
            UnlockMutex: None,
            flags: 0,
            pReserved: CString::new(reserved).unwrap().into_raw()
                as *mut std::ffi::c_void,
        }
    }

    fn finalize(&mut self) {
        if self.finalize.is_none() {
            self.sync = None;
            /* wait until we can read, which means the winner finalized the module */
            drop(FINI.read().unwrap());
        } else {
            self.sync = None;
            /* wait for all others to complete */
            drop(SYNC.write().unwrap());
            let ret = fn_finalize(std::ptr::null_mut());
            assert_eq!(ret, CKR_OK);
            /* winner finalized and completed the tests */
            self.finalize = None;
        }
        if self.created {
            std::fs::remove_file(self.filename).unwrap_or(());
        }
    }
}

#[test]
fn test_token() {
    let mut testdata = TestData::new("testdata/test_token.json");
    testdata.setup_db();

    let mut plist: *mut CK_FUNCTION_LIST = std::ptr::null_mut();
    let pplist = &mut plist;
    let result = C_GetFunctionList(&mut *pplist);
    assert_eq!(result, 0);
    unsafe {
        let list: CK_FUNCTION_LIST = *plist;
        match list.C_Initialize {
            Some(value) => {
                let mut args = testdata.make_init_args();
                let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
                let ret = value(args_ptr as *mut std::ffi::c_void);
                assert_eq!(ret, CKR_OK)
            }
            None => todo!(),
        }
    }

    testdata.finalize();
}

#[test]
fn test_random() {
    let mut testdata = TestData::new("testdata/test_random.json");
    testdata.setup_db();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);
    let mut handle: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);
    let data: &[u8] = &mut [0, 0, 0, 0];
    ret = fn_generate_random(
        handle,
        data.as_ptr() as *mut u8,
        data.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);
    assert_ne!(data, &[0, 0, 0, 0]);
    ret = fn_close_session(handle);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_login() {
    let mut testdata = TestData::new("testdata/test_login.json");
    testdata.setup_db();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);

    let mut handle: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);

    let mut info = CK_SESSION_INFO {
        slotID: CK_UNAVAILABLE_INFORMATION,
        state: CK_UNAVAILABLE_INFORMATION,
        flags: 0,
        ulDeviceError: 0,
    };
    ret = fn_get_session_info(handle, &mut info);
    assert_eq!(ret, CKR_OK);
    assert_eq!(info.state, CKS_RO_PUBLIC_SESSION);

    let mut handle2: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION | CKF_RW_SESSION,
        std::ptr::null_mut(),
        None,
        &mut handle2,
    );
    assert_eq!(ret, CKR_OK);
    ret = fn_get_session_info(handle2, &mut info);
    assert_eq!(ret, CKR_OK);
    assert_eq!(info.state, CKS_RW_PUBLIC_SESSION);

    /* login */
    let pin = "12345678";
    ret = fn_login(
        handle,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    ret = fn_get_session_info(handle, &mut info);
    assert_eq!(ret, CKR_OK);
    assert_eq!(info.state, CKS_RO_USER_FUNCTIONS);

    ret = fn_get_session_info(handle2, &mut info);
    assert_eq!(ret, CKR_OK);
    assert_eq!(info.state, CKS_RW_USER_FUNCTIONS);

    ret = fn_login(
        handle,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_USER_ALREADY_LOGGED_IN);

    ret = fn_logout(handle2);
    assert_eq!(ret, CKR_OK);

    ret = fn_get_session_info(handle, &mut info);
    assert_eq!(ret, CKR_OK);
    assert_eq!(info.state, CKS_RO_PUBLIC_SESSION);

    ret = fn_get_session_info(handle2, &mut info);
    assert_eq!(ret, CKR_OK);
    assert_eq!(info.state, CKS_RW_PUBLIC_SESSION);

    ret = fn_logout(handle);
    assert_eq!(ret, CKR_USER_NOT_LOGGED_IN);

    ret = fn_close_session(handle);
    assert_eq!(ret, CKR_OK);
    ret = fn_close_session(handle2);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_get_attr() {
    let mut testdata = TestData::new("testdata/test_get_attr.json");
    testdata.setup_db();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);
    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    let mut template = Vec::<CK_ATTRIBUTE>::new();
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;

    /* public key data */
    template.push(make_attribute!(
        CKA_UNIQUE_ID,
        CString::new("2").unwrap().into_raw(),
        1
    ));
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    assert_eq!(count, 1);
    assert_ne!(handle, CK_INVALID_HANDLE);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    template.clear();
    template.push(make_attribute!(CKA_LABEL, std::ptr::null_mut(), 0));

    ret = fn_get_attribute_value(session, handle, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    assert_ne!(template[0].ulValueLen, 0);

    let data: &mut [u8] = &mut [0; 128];
    template[0].pValue = data.as_ptr() as *mut std::ffi::c_void;
    template[0].ulValueLen = 128;

    ret = fn_get_attribute_value(session, handle, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let size = template[0].ulValueLen as usize;
    let value = std::str::from_utf8(&data[0..size]).unwrap();
    assert_eq!(value, "Test RSA Key");

    template[0].ulValueLen = 1;
    ret = fn_get_attribute_value(session, handle, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_BUFFER_TOO_SMALL);

    /* private key data */
    template.clear();
    handle = CK_INVALID_HANDLE;
    template.push(make_attribute!(
        CKA_UNIQUE_ID,
        CString::new("3").unwrap().into_raw(),
        1
    ));
    /* first try should not find it */
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    assert_eq!(count, 0);
    assert_eq!(handle, CK_INVALID_HANDLE);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    /* login */
    let pin = "12345678";
    ret = fn_login(
        session,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* after login should find it */
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    assert_eq!(count, 1);
    assert_ne!(handle, CK_INVALID_HANDLE);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    template.clear();
    template.push(make_attribute!(
        CKA_PRIVATE_EXPONENT,
        (&mut [0; 128]).as_ptr(),
        128
    ));

    /* should fail for sensitive attributes */
    ret = fn_get_attribute_value(session, handle, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_ATTRIBUTE_SENSITIVE);

    /* and succeed for public ones */
    template[0].type_ = CKA_PUBLIC_EXPONENT;
    template[0].ulValueLen = 128;
    ret = fn_get_attribute_value(session, handle, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    assert_eq!(template[0].ulValueLen, 3);

    ret = fn_logout(session);
    assert_eq!(ret, CKR_OK);
    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_set_attr() {
    let mut testdata = TestData::new("testdata/test_set_attr.json");
    testdata.setup_db();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);
    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    let mut template = Vec::<CK_ATTRIBUTE>::new();
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;

    /* public key data */
    template.push(make_attribute!(
        CKA_UNIQUE_ID,
        CString::new("2").unwrap().into_raw(),
        1
    ));
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    assert_eq!(count, 1);
    assert_ne!(handle, CK_INVALID_HANDLE);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    let label = "new label";
    template.clear();
    template.push(make_attribute!(CKA_LABEL, label.as_ptr(), label.len()));
    ret = fn_set_attribute_value(session, handle, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_USER_NOT_LOGGED_IN);

    /* login */
    let pin = "12345678";
    ret = fn_login(
        session,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    ret = fn_set_attribute_value(session, handle, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_SESSION_READ_ONLY);

    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION | CKF_RW_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    ret = fn_set_attribute_value(session, handle, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);

    let unique = "unsettable";
    template.clear();
    template.push(make_attribute!(
        CKA_UNIQUE_ID,
        unique.as_ptr(),
        unique.len()
    ));

    ret = fn_set_attribute_value(session, handle, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_ATTRIBUTE_READ_ONLY);

    ret = fn_logout(session);
    assert_eq!(ret, CKR_OK);
    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_copy_objects() {
    let mut testdata = TestData::new("testdata/test_copy_objects.json");
    testdata.setup_db();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);
    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    /* login */
    let pin = "12345678";
    ret = fn_login(
        session,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* public key data */
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;
    let mut template = vec![make_attribute!(
        CKA_UNIQUE_ID,
        CString::new("2").unwrap().into_raw(),
        1
    )];
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    assert_eq!(count, 1);
    assert_ne!(handle, CK_INVALID_HANDLE);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    /* copy token object to session object */
    let mut intoken: CK_BBOOL = CK_FALSE;
    let mut private: CK_BBOOL = CK_TRUE;
    let mut template = vec![
        make_attribute!(CKA_TOKEN, &mut intoken as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_PRIVATE, &mut private as *mut _, CK_BBOOL_SIZE),
    ];
    let mut handle2: CK_ULONG = CK_INVALID_HANDLE;
    ret = fn_copy_object(
        session,
        handle,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle2,
    );
    assert_eq!(ret, CKR_OK);

    /* make not copyable object */
    let mut class = CKO_DATA;
    let mut copyable: CK_BBOOL = CK_FALSE;
    let application = "nocopy";
    let data = "data";
    let mut template = vec![
        make_attribute!(CKA_CLASS, &mut class as *mut _, CK_ULONG_SIZE),
        make_attribute!(CKA_COPYABLE, &mut copyable as *mut _, CK_BBOOL_SIZE),
        make_attribute!(
            CKA_APPLICATION,
            CString::new(application).unwrap().into_raw(),
            application.len()
        ),
        make_attribute!(
            CKA_VALUE,
            CString::new(data).unwrap().into_raw(),
            data.len()
        ),
    ];
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;
    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);

    /* copy token object to session object */
    let mut intoken: CK_BBOOL = CK_FALSE;
    let mut template = vec![make_attribute!(
        CKA_TOKEN,
        &mut intoken as *mut _,
        CK_BBOOL_SIZE
    )];
    let mut handle2: CK_ULONG = CK_INVALID_HANDLE;
    ret = fn_copy_object(
        session,
        handle,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle2,
    );
    assert_eq!(ret, CKR_ACTION_PROHIBITED);

    ret = fn_logout(session);
    assert_eq!(ret, CKR_OK);
    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_create_objects() {
    let mut testdata = TestData::new("testdata/test_create_objects.json");
    testdata.setup_db();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);
    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    let mut class = CKO_DATA;
    let application = "test";
    let data = "payload";
    let mut template = vec![
        make_attribute!(CKA_CLASS, &mut class as *mut _, CK_ULONG_SIZE),
        make_attribute!(
            CKA_APPLICATION,
            CString::new(application).unwrap().into_raw(),
            application.len()
        ),
        make_attribute!(
            CKA_VALUE,
            CString::new(data).unwrap().into_raw(),
            data.len()
        ),
    ];

    let mut handle: CK_ULONG = CK_INVALID_HANDLE;
    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_USER_NOT_LOGGED_IN);

    /* login */
    let pin = "12345678";
    ret = fn_login(
        session,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);

    let mut intoken: CK_BBOOL = CK_TRUE;
    template.push(make_attribute!(
        CKA_TOKEN,
        &mut intoken as *mut _,
        CK_BBOOL_SIZE
    ));

    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_SESSION_READ_ONLY);

    let login_session = session;

    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION | CKF_RW_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);

    class = CKO_CERTIFICATE;
    let mut ctype = CKC_X_509;
    let mut trusted: CK_BBOOL = CK_FALSE;
    let ignored = "ignored";
    let bogus = "bogus";
    template = vec![
        make_attribute!(CKA_CLASS, &mut class as *mut _, CK_ULONG_SIZE),
        make_attribute!(CKA_TOKEN, &mut intoken as *mut _, CK_BBOOL_SIZE),
        make_attribute!(
            CKA_CERTIFICATE_TYPE,
            &mut ctype as *mut _,
            CK_ULONG_SIZE
        ),
        make_attribute!(CKA_TRUSTED, &mut trusted as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_CHECK_VALUE, ignored.as_ptr(), 42),
        make_attribute!(CKA_SUBJECT, bogus.as_ptr(), bogus.len()),
        make_attribute!(CKA_VALUE, bogus.as_ptr(), bogus.len()),
    ];

    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);

    class = CKO_PUBLIC_KEY;
    let mut ktype = CKK_RSA;
    let mut encrypt: CK_BBOOL = CK_TRUE;
    let label = "RSA Public Encryption Key";
    let modulus_hex = "9D2E7820CE719B9194CDFE0FD751214193C4E9BE9BFA24D0E91B0FC3541C85885CB3CA95F8FDA4E129558EE41F653481E66A04ECB75808D57BD76ED9069767A2AFC9C3188F2BD42F045D0575765ADE27AD033B338DD5C2C1AAA899B89201A34BBB6ED9CCD0511325ADCF1C69718BD27196447D567F17E35A5865A3BC1FB35B3A605C25294D2A02E5F53D170C57814D8246F50CAE32321D8A5C44508238AC50519BD12221C740620198B762C2D1670A4B94655C783EAAD0E9A1244F8AE86D3B4A3DF26AC532B6A4EAA4FB4A35DF5C3A1B755DC5C17E451643D2DB722113C1E3E2CA59CFA592C80FB9B2D7056E19F5C84198371465CE7DFBA7390C3CE19D878121";
    let modulus =
        hex::decode(modulus_hex).expect("Failed to decode hex modulus");
    let exponent = hex::decode("010001").expect("Failed to decode exponent");
    template = vec![
        make_attribute!(CKA_CLASS, &mut class as *mut _, CK_ULONG_SIZE),
        make_attribute!(CKA_KEY_TYPE, &mut ktype as *mut _, CK_ULONG_SIZE),
        make_attribute!(CKA_ENCRYPT, &mut encrypt as *mut _, CK_BBOOL_SIZE),
        make_attribute!(
            CKA_LABEL,
            label.as_ptr() as *mut std::ffi::c_void,
            label.len()
        ),
        make_attribute!(
            CKA_MODULUS,
            modulus.as_ptr() as *mut std::ffi::c_void,
            modulus.len()
        ),
        make_attribute!(
            CKA_PUBLIC_EXPONENT,
            exponent.as_ptr() as *mut std::ffi::c_void,
            exponent.len()
        ),
    ];

    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);

    /* Private RSA Key with missing Q,A,B parameters */
    class = CKO_PRIVATE_KEY;
    let mut ktype = CKK_RSA;
    let mut encrypt: CK_BBOOL = CK_TRUE;
    let label = "RSA Private Key";
    let modulus = hex::decode("9d2e7820ce719b9194cdfe0fd751214193c4e9be9bfa24d0e91b0fc3541c85885cb3ca95f8fda4e129558ee41f653481e66a04ecb75808d57bd76ed9069767a2afc9c3188f2bd42f045d0575765ade27ad033b338dd5c2c1aaa899b89201a34bbb6ed9ccd0511325adcf1c69718bd27196447d567f17e35a5865a3bc1fb35b3a605c25294d2a02e5f53d170c57814d8246f50cae32321d8a5c44508238ac50519bd12221c740620198b762c2d1670a4b94655c783eaad0e9a1244f8ae86d3b4a3df26ac532b6a4eaa4fb4a35df5c3a1b755dc5c17e451643d2db722113c1e3e2ca59cfa592c80fb9b2d7056e19f5c84198371465ce7dfba7390c3ce19d878121").expect("Failed to decode modulus");
    let pub_exponent =
        hex::decode("010001").expect("Failed to decode public exponent");
    let pri_exponent = hex::decode("14537d0f690302062a8314f6c17669618c956b50cde4e43bebd92709b067dbd0cd84268f8c5a68a7016c62051816435b050bf2c515d49997d9e2fb1faf9d86b6601b2c5291b92e404245313e8666abd1dfaaca4e196a6a3c1730a4685ce13f57bcce51f60d7e5e8681da85a7111aeec4e794c5cc98b4e31ebccdb005d4e7a1c54fcb81eb28a16d649489dfb2374bd3fbcf8e7e68197c08ed48601daa3512367961f4e8ba9a0ecae868365034ac1bba9accdfd0db0407142da7ea1a2b2e4c70e57707ac0db0b9b93f92b9839e5ce0dc61b4a804b60043f9f07675eb6e91eb029767c495682a9261344f9c825d22c148a9d2205d0fa5c521fadf8abbfae75fe591").expect("Failed to decode private exponent");
    let prime_1 = hex::decode("00d76285da69d58f6bca20e85cd645ea5fca42d872e92f190b7cc76cf50d2903ba213a8599db5429dd429a938376b64085bd9e8dd56360470d0d06684a3c18536c4929b3ba7b5f4848ec49327c2094afdd22e66eadf4f6e1af6456e49b4b0f0155c007003d4da785296f49ae013b509c918cc76b48f197a13a67e5eb11f883f585").expect("Failed to decode prime 1");
    let coefficient = hex::decode("26ee312416332f9b8e7c0ab1d0dcc3d7edaea735ffc43295efa876d1948991fd49f2f2a1a54e99ee13ea79903acc48520f0c4b5129687cf5efae60982f1848d54c490a452550d90bb68205d9f350f7134651c84ac9869047c455d1f0f31d6a3a6761ecab2e326190cedd65f775147dae147f1ec7d679cd198fc2a62422fb6178").expect("Failed to decode prime 1");
    template = vec![
        make_attribute!(CKA_CLASS, &mut class as *mut _, CK_ULONG_SIZE),
        make_attribute!(CKA_KEY_TYPE, &mut ktype as *mut _, CK_ULONG_SIZE),
        make_attribute!(CKA_SIGN, &mut encrypt as *mut _, CK_BBOOL_SIZE),
        make_attribute!(
            CKA_LABEL,
            label.as_ptr() as *mut std::ffi::c_void,
            label.len()
        ),
        make_attribute!(
            CKA_MODULUS,
            modulus.as_ptr() as *mut std::ffi::c_void,
            modulus.len()
        ),
        make_attribute!(
            CKA_PUBLIC_EXPONENT,
            pub_exponent.as_ptr() as *mut std::ffi::c_void,
            pub_exponent.len()
        ),
        make_attribute!(
            CKA_PRIVATE_EXPONENT,
            pri_exponent.as_ptr() as *mut std::ffi::c_void,
            pri_exponent.len()
        ),
        make_attribute!(
            CKA_PRIME_1,
            prime_1.as_ptr() as *mut std::ffi::c_void,
            prime_1.len()
        ),
        make_attribute!(
            CKA_COEFFICIENT,
            coefficient.as_ptr() as *mut std::ffi::c_void,
            coefficient.len()
        ),
    ];

    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);

    /* Test create secret key object */
    class = CKO_SECRET_KEY;
    ktype = CKK_GENERIC_SECRET;
    let label = "Test Generic Secret";
    let value = "Anything";
    template = vec![
        make_attribute!(CKA_CLASS, &mut class as *mut _, CK_ULONG_SIZE),
        make_attribute!(CKA_KEY_TYPE, &mut ktype as *mut _, CK_ULONG_SIZE),
        make_attribute!(
            CKA_LABEL,
            label.as_ptr() as *mut std::ffi::c_void,
            label.len()
        ),
        make_attribute!(
            CKA_VALUE,
            value.as_ptr() as *mut std::ffi::c_void,
            value.len()
        ),
    ];

    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);

    let mut size: CK_ULONG = 0;
    ret = fn_get_object_size(session, handle, &mut size);
    assert_eq!(ret, CKR_OK);
    assert_ne!(size, 0);

    ret = fn_destroy_object(session, handle);
    assert_eq!(ret, CKR_OK);

    ret = fn_logout(login_session);
    assert_eq!(ret, CKR_OK);
    ret = fn_close_session(login_session);
    assert_eq!(ret, CKR_OK);
    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_init_token() {
    let mut testdata = TestData::new("testdata/test_init_token.json");
    /* skip setup, we are creating an unitiliaized token */

    /* but mark the file as created,
     * so it will be cleaned up when the test is complete */
    testdata.mark_file_created();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);
    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    let mut ro_session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;

    /* init once */
    let pin_value = "SO Pin Value";
    ret = fn_init_token(
        testdata.get_slot(),
        CString::new(pin_value).unwrap().into_raw() as *mut u8,
        pin_value.len() as CK_ULONG,
        std::ptr::null_mut(),
    );
    assert_eq!(ret, CKR_OK);

    /* verify wrong SO PIN fails */
    let bad_value = "SO Bad Value";
    ret = fn_init_token(
        testdata.get_slot(),
        CString::new(bad_value).unwrap().into_raw() as *mut u8,
        pin_value.len() as CK_ULONG,
        std::ptr::null_mut(),
    );
    assert_eq!(ret, CKR_PIN_INCORRECT);

    /* re-init */
    let pin_value = "SO Pin Value";
    ret = fn_init_token(
        testdata.get_slot(),
        CString::new(pin_value).unwrap().into_raw() as *mut u8,
        pin_value.len() as CK_ULONG,
        std::ptr::null_mut(),
    );
    assert_eq!(ret, CKR_OK);

    /* login as so */
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION | CKF_RW_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);
    ret = fn_login(
        session,
        CKU_SO,
        CString::new(pin_value).unwrap().into_raw() as *mut u8,
        pin_value.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* change so pin */
    let new_pin = "New SO Pin Value";
    ret = fn_set_pin(
        session,
        CString::new(pin_value).unwrap().into_raw() as *mut u8,
        pin_value.len() as CK_ULONG,
        CString::new(new_pin).unwrap().into_raw() as *mut u8,
        new_pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* try to open ro_session and fail */
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut ro_session,
    );
    assert_eq!(ret, CKR_SESSION_READ_WRITE_SO_EXISTS);

    /* logout and retry */
    ret = fn_logout(session);
    assert_eq!(ret, CKR_OK);
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut ro_session,
    );
    assert_eq!(ret, CKR_OK);

    /* try to change pin and fail with ro_session */
    ret = fn_set_pin(
        ro_session,
        CString::new(pin_value).unwrap().into_raw() as *mut u8,
        pin_value.len() as CK_ULONG,
        CString::new(new_pin).unwrap().into_raw() as *mut u8,
        new_pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_SESSION_READ_ONLY);

    /* try to login again and fail because of ro_session exists */
    ret = fn_login(
        session,
        CKU_SO,
        CString::new(new_pin).unwrap().into_raw() as *mut u8,
        new_pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_SESSION_READ_ONLY_EXISTS);

    /* try again after closing ro_session */
    ret = fn_close_session(ro_session);
    assert_eq!(ret, CKR_OK);
    ret = fn_login(
        session,
        CKU_SO,
        CString::new(new_pin).unwrap().into_raw() as *mut u8,
        new_pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* set user pin */
    let user_pin = "User PIN Value";
    ret = fn_init_pin(
        session,
        CString::new(user_pin).unwrap().into_raw() as *mut u8,
        user_pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* try to log in as user and fail because SO active */
    ret = fn_login(
        session,
        CKU_USER,
        CString::new(user_pin).unwrap().into_raw() as *mut u8,
        user_pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_USER_ANOTHER_ALREADY_LOGGED_IN);

    /* retry user login after logout */
    ret = fn_logout(session);
    assert_eq!(ret, CKR_OK);
    ret = fn_login(
        session,
        CKU_USER,
        CString::new(user_pin).unwrap().into_raw() as *mut u8,
        user_pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* change user pin as user */
    let new_user_pin = "New User PIN Value";
    ret = fn_set_pin(
        session,
        CString::new(user_pin).unwrap().into_raw() as *mut u8,
        user_pin.len() as CK_ULONG,
        CString::new(new_user_pin).unwrap().into_raw() as *mut u8,
        new_user_pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    ret = fn_logout(session);
    assert_eq!(ret, CKR_OK);

    /* change back user pin after logout */
    ret = fn_set_pin(
        session,
        CString::new(new_user_pin).unwrap().into_raw() as *mut u8,
        new_user_pin.len() as CK_ULONG,
        CString::new(user_pin).unwrap().into_raw() as *mut u8,
        user_pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    ret = fn_logout(session);
    assert_eq!(ret, CKR_OK);
    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_get_mechs() {
    let mut testdata = TestData::new("testdata/test_get_mechs.json");
    testdata.setup_db();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_get_mechanism_list(
        testdata.get_slot(),
        std::ptr::null_mut(),
        &mut count,
    );
    assert_eq!(ret, CKR_OK);
    let mut mechs: Vec<CK_MECHANISM_TYPE> = vec![0; count as usize];
    ret = fn_get_mechanism_list(
        testdata.get_slot(),
        mechs.as_mut_ptr() as CK_MECHANISM_TYPE_PTR,
        &mut count,
    );
    assert_eq!(ret, CKR_OK);
    assert_eq!(true, count > 4);
    let mut info: CK_MECHANISM_INFO = Default::default();
    ret = fn_get_mechanism_info(testdata.get_slot(), mechs[0], &mut info);
    assert_eq!(ret, CKR_OK);
    assert_eq!(info.ulMinKeySize, 1024);

    testdata.finalize();
}

#[test]
fn test_aes_operations() {
    let mut testdata = TestData::new("testdata/test_aes_operations.json");
    testdata.setup_db();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);

    /* open session */
    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION | CKF_RW_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    /* login */
    let pin = "12345678";
    ret = fn_login(
        session,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* Generate AES key */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_AES_KEY_GEN,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };

    let mut handle: CK_ULONG = CK_INVALID_HANDLE;

    let mut class = CKO_SECRET_KEY;
    let mut len: CK_ULONG = 16;
    let mut truebool = CK_TRUE;
    let mut template = vec![
        make_attribute!(CKA_CLASS, &mut class as *mut _, CK_ULONG_SIZE),
        make_attribute!(CKA_VALUE_LEN, &mut len as *mut _, CK_ULONG_SIZE),
        make_attribute!(CKA_SENSITIVE, &mut truebool as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_TOKEN, &mut truebool as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_ENCRYPT, &mut truebool as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_DECRYPT, &mut truebool as *mut _, CK_BBOOL_SIZE),
    ];

    ret = fn_generate_key(
        session,
        &mut mechanism,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);

    {
        /* AES ECB */

        /* encrypt init */
        let mut mechanism: CK_MECHANISM = CK_MECHANISM {
            mechanism: CKM_AES_ECB,
            pParameter: std::ptr::null_mut(),
            ulParameterLen: 0,
        };

        ret = fn_encrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        /* Data need to be exactly one block in size */
        let data = "0123456789ABCDEF";
        let enc: [u8; 16] = [0; 16];
        let mut enc_len: CK_ULONG = 16;
        ret = fn_encrypt(
            session,
            CString::new(data).unwrap().into_raw() as *mut u8,
            data.len() as CK_ULONG,
            enc.as_ptr() as *mut _,
            &mut enc_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(enc_len, 16);

        ret = fn_decrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        let dec: [u8; 16] = [0; 16];
        let mut dec_len: CK_ULONG = 16;
        ret = fn_decrypt(
            session,
            enc.as_ptr() as *mut _,
            enc_len,
            dec.as_ptr() as *mut _,
            &mut dec_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(dec_len as usize, data.len());
        assert_eq!(data.as_bytes(), &dec);
    }

    {
        /* AES CBC */

        /* encrypt init */
        let iv = "FEDCBA0987654321";
        let mut mechanism: CK_MECHANISM = CK_MECHANISM {
            mechanism: CKM_AES_CBC,
            pParameter: CString::new(iv).unwrap().into_raw() as CK_VOID_PTR,
            ulParameterLen: iv.len() as CK_ULONG,
        };

        ret = fn_encrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        /* Data need to be exactly one block in size */
        let data = "0123456789ABCDEF";
        let enc: [u8; 16] = [0; 16];
        let mut enc_len: CK_ULONG = 16;
        ret = fn_encrypt(
            session,
            CString::new(data).unwrap().into_raw() as *mut u8,
            data.len() as CK_ULONG,
            enc.as_ptr() as *mut _,
            &mut enc_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(enc_len, 16);

        ret = fn_decrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        let dec: [u8; 16] = [0; 16];
        let mut dec_len: CK_ULONG = 16;
        ret = fn_decrypt(
            session,
            enc.as_ptr() as *mut _,
            enc_len,
            dec.as_ptr() as *mut _,
            &mut dec_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(dec_len as usize, data.len());
        assert_eq!(data.as_bytes(), &dec);
    }

    {
        /* AES CBC and Padding */

        /* encrypt init */
        let iv = "FEDCBA0987654321";
        let mut mechanism: CK_MECHANISM = CK_MECHANISM {
            mechanism: CKM_AES_CBC_PAD,
            pParameter: CString::new(iv).unwrap().into_raw() as CK_VOID_PTR,
            ulParameterLen: iv.len() as CK_ULONG,
        };

        ret = fn_encrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        /* Data of exactly one block in size will cause two blocks output */
        let data = "0123456789ABCDEF";
        let enc: [u8; 32] = [0; 32];
        let mut enc_len: CK_ULONG = 32;
        ret = fn_encrypt(
            session,
            CString::new(data).unwrap().into_raw() as *mut u8,
            data.len() as CK_ULONG,
            enc.as_ptr() as *mut _,
            &mut enc_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(enc_len, 32);

        ret = fn_decrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        let dec: [u8; 32] = [0; 32];
        let mut dec_len: CK_ULONG = 32;
        ret = fn_decrypt(
            session,
            enc.as_ptr() as *mut _,
            enc_len,
            dec.as_ptr() as *mut _,
            &mut dec_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(dec_len as usize, data.len());
        assert_eq!(data.as_bytes(), &dec[..dec_len as usize])
    }

    #[cfg(not(feature = "fips"))]
    {
        /* AES OFB */

        /* encrypt init */
        let iv = "FEDCBA0987654321";
        let mut mechanism: CK_MECHANISM = CK_MECHANISM {
            mechanism: CKM_AES_OFB,
            pParameter: CString::new(iv).unwrap().into_raw() as CK_VOID_PTR,
            ulParameterLen: iv.len() as CK_ULONG,
        };

        ret = fn_encrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        /* Stream mode, so arbitrary data size and matching output */
        let data = "01234567";
        let enc: [u8; 16] = [0; 16];
        let mut enc_len: CK_ULONG = 16;
        ret = fn_encrypt(
            session,
            CString::new(data).unwrap().into_raw() as *mut u8,
            data.len() as CK_ULONG,
            enc.as_ptr() as *mut _,
            &mut enc_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(enc_len as usize, data.len());

        ret = fn_decrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        let dec: [u8; 16] = [0; 16];
        let mut dec_len: CK_ULONG = 16;
        ret = fn_decrypt(
            session,
            enc.as_ptr() as *mut _,
            enc_len,
            dec.as_ptr() as *mut _,
            &mut dec_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(dec_len as usize, data.len());
        assert_eq!(data.as_bytes(), &dec[..dec_len as usize])
    }

    #[cfg(not(feature = "fips"))]
    {
        /* AES CFB */

        /* encrypt init */
        let iv = "FEDCBA0987654321";
        let mut mechanism: CK_MECHANISM = CK_MECHANISM {
            mechanism: CKM_AES_CFB1,
            pParameter: CString::new(iv).unwrap().into_raw() as CK_VOID_PTR,
            ulParameterLen: iv.len() as CK_ULONG,
        };

        ret = fn_encrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        /* Stream mode, so arbitrary data size and matching output */
        let data = "01234567";
        let enc: [u8; 16] = [0; 16];
        let mut enc_len: CK_ULONG = 16;
        ret = fn_encrypt(
            session,
            CString::new(data).unwrap().into_raw() as *mut u8,
            data.len() as CK_ULONG,
            enc.as_ptr() as *mut _,
            &mut enc_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(enc_len as usize, data.len());

        ret = fn_decrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        let dec: [u8; 16] = [0; 16];
        let mut dec_len: CK_ULONG = 16;
        ret = fn_decrypt(
            session,
            enc.as_ptr() as *mut _,
            enc_len,
            dec.as_ptr() as *mut _,
            &mut dec_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(dec_len as usize, data.len());
        assert_eq!(data.as_bytes(), &dec[..dec_len as usize])
    }

    {
        /* AES CTR */

        /* encrypt init */
        let mut param = CK_AES_CTR_PARAMS {
            ulCounterBits: 128,
            cb: [
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
                0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
            ],
        };

        let mut mechanism: CK_MECHANISM = CK_MECHANISM {
            mechanism: CKM_AES_CTR,
            pParameter: &mut param as *mut CK_AES_CTR_PARAMS as CK_VOID_PTR,
            ulParameterLen: std::mem::size_of::<CK_AES_CTR_PARAMS>()
                as CK_ULONG,
        };

        ret = fn_encrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        /* Stream mode, so arbitrary data size and matching output */
        let data = "01234567";
        let enc: [u8; 16] = [0; 16];
        let mut enc_len: CK_ULONG = 16;
        ret = fn_encrypt(
            session,
            CString::new(data).unwrap().into_raw() as *mut u8,
            data.len() as CK_ULONG,
            enc.as_ptr() as *mut _,
            &mut enc_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(enc_len as usize, data.len());

        ret = fn_decrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        let dec: [u8; 16] = [0; 16];
        let mut dec_len: CK_ULONG = 16;
        ret = fn_decrypt(
            session,
            enc.as_ptr() as *mut _,
            enc_len,
            dec.as_ptr() as *mut _,
            &mut dec_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(dec_len as usize, data.len());
        assert_eq!(data.as_bytes(), &dec[..dec_len as usize]);

        /* Counterbits edge cases */

        /* 9 bit counter, counter value should allow a singe block before
         * wrap around */
        let mut param = CK_AES_CTR_PARAMS {
            ulCounterBits: 9,
            cb: [
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x01, 0xFE,
            ],
        };

        let mut mechanism: CK_MECHANISM = CK_MECHANISM {
            mechanism: CKM_AES_CTR,
            pParameter: &mut param as *mut CK_AES_CTR_PARAMS as CK_VOID_PTR,
            ulParameterLen: std::mem::size_of::<CK_AES_CTR_PARAMS>()
                as CK_ULONG,
        };

        ret = fn_encrypt_init(session, &mut mechanism, handle);
        assert_eq!(ret, CKR_OK);

        /* Stream mode, so arbitrary data size and matching output */
        let mut data: [u8; 16] = [255u8; 16];
        let enc: [u8; 16] = [0; 16];
        let mut enc_len: CK_ULONG = 16;

        /* First block should suceed */
        ret = fn_encrypt_update(
            session,
            data.as_mut_ptr(),
            data.len() as CK_ULONG,
            enc.as_ptr() as *mut _,
            &mut enc_len,
        );
        assert_eq!(ret, CKR_OK);
        assert_eq!(enc_len as usize, data.len());

        /* Second should fail */
        ret = fn_encrypt_update(
            session,
            data.as_mut_ptr(),
            data.len() as CK_ULONG,
            enc.as_ptr() as *mut _,
            &mut enc_len,
        );
        assert_eq!(ret, CKR_DATA_LEN_RANGE);
    }

    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_rsa_operations() {
    let mut testdata = TestData::new("testdata/test_rsa_operations.json");

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);

    /* open session */
    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    /* login */
    let pin = "12345678";
    ret = fn_login(
        session,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* public key data */
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;
    let mut template = vec![make_attribute!(
        CKA_UNIQUE_ID,
        CString::new("2").unwrap().into_raw(),
        1
    )];
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    assert_eq!(count, 1);
    assert_ne!(handle, CK_INVALID_HANDLE);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    /* encrypt init */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_RSA_PKCS,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    ret = fn_encrypt_init(session, &mut mechanism, handle);
    assert_eq!(ret, CKR_OK);

    /* a second invocation should return an error */
    ret = fn_encrypt_init(session, &mut mechanism, handle);
    assert_eq!(ret, CKR_OPERATION_ACTIVE);

    let data = "plaintext";
    let enc: [u8; 512] = [0; 512];
    let mut enc_len: CK_ULONG = 512;
    ret = fn_encrypt(
        session,
        CString::new(data).unwrap().into_raw() as *mut u8,
        data.len() as CK_ULONG,
        enc.as_ptr() as *mut _,
        &mut enc_len,
    );
    assert_eq!(ret, CKR_OK);
    assert_eq!(enc_len, 256);

    /* a second invocation should return an error */
    ret = fn_encrypt(
        session,
        CString::new(data).unwrap().into_raw() as *mut u8,
        data.len() as CK_ULONG,
        enc.as_ptr() as *mut _,
        &mut enc_len,
    );
    assert_eq!(ret, CKR_OPERATION_NOT_INITIALIZED);

    /* test that decryption returns the same data back */
    template = vec![make_attribute!(
        CKA_UNIQUE_ID,
        CString::new("3").unwrap().into_raw(),
        1
    )];
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    assert_eq!(count, 1);
    assert_ne!(handle, CK_INVALID_HANDLE);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    ret = fn_decrypt_init(session, &mut mechanism, handle);
    assert_eq!(ret, CKR_OK);

    let dec: [u8; 512] = [0; 512];
    let mut dec_len: CK_ULONG = 512;
    ret = fn_decrypt(
        session,
        enc.as_ptr() as *mut u8,
        enc_len,
        dec.as_ptr() as *mut u8,
        &mut dec_len,
    );
    assert_eq!(ret, CKR_OK);
    assert_eq!(data.as_bytes(), &dec[..dec_len as usize]);

    ret = fn_logout(session);
    assert_eq!(ret, CKR_OK);
    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_session_objects() {
    let mut testdata = TestData::new("testdata/test_session_objects.json");
    testdata.setup_db();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);
    let mut login_session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut login_session,
    );
    assert_eq!(ret, CKR_OK);

    /* login */
    let pin = "12345678";
    ret = fn_login(
        login_session,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION | CKF_RW_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    /* ephemeral session object */
    let mut class = CKO_DATA;
    let app1 = "app1";
    let data1 = "session data";
    let mut template = vec![
        make_attribute!(CKA_CLASS, &mut class as *mut _, CK_ULONG_SIZE),
        make_attribute!(
            CKA_APPLICATION,
            CString::new(app1).unwrap().into_raw(),
            app1.len()
        ),
        make_attribute!(
            CKA_VALUE,
            CString::new(data1).unwrap().into_raw(),
            data1.len()
        ),
    ];

    let mut handle1: CK_ULONG = CK_INVALID_HANDLE;
    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle1,
    );
    assert_eq!(ret, CKR_OK);

    /* store in token object */
    let mut intoken: CK_BBOOL = CK_TRUE;
    let app2 = "app2";
    let data2 = "token data";
    let mut template = vec![
        make_attribute!(CKA_CLASS, &mut class as *mut _, CK_ULONG_SIZE),
        make_attribute!(
            CKA_APPLICATION,
            CString::new(app2).unwrap().into_raw(),
            app2.len()
        ),
        make_attribute!(
            CKA_VALUE,
            CString::new(data2).unwrap().into_raw(),
            data2.len()
        ),
        make_attribute!(CKA_TOKEN, &mut intoken as *mut _, CK_BBOOL_SIZE),
    ];

    let mut handle2: CK_ULONG = CK_INVALID_HANDLE;
    ret = fn_create_object(
        session,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle2,
    );
    assert_eq!(ret, CKR_OK);

    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION | CKF_RW_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    /* check that the session object handle invalid now */
    template.clear();
    template.push(make_attribute!(CKA_VALUE, std::ptr::null_mut(), 0));

    ret = fn_get_attribute_value(session, handle1, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OBJECT_HANDLE_INVALID);

    /* check that the session object is gone */
    template.clear();
    template.push(make_attribute!(
        CKA_APPLICATION,
        CString::new(app1).unwrap().into_raw(),
        app1.len()
    ));

    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle1, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    assert_eq!(count, 0);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    /* check that the token object is there */
    template.clear();
    template.push(make_attribute!(
        CKA_APPLICATION,
        CString::new(app2).unwrap().into_raw(),
        app2.len()
    ));

    handle2 = CK_INVALID_HANDLE;
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    ret = fn_find_objects(session, &mut handle2, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    assert_eq!(count, 1);
    assert_ne!(handle2, CK_INVALID_HANDLE);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    ret = fn_logout(login_session);
    assert_eq!(ret, CKR_OK);
    ret = fn_close_session(login_session);
    assert_eq!(ret, CKR_OK);
    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_hashes_digest() {
    let mut testdata = TestData::new("testdata/test_hashes.json");

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);

    /* open session */
    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    /* get test data */
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;
    let mut template = vec![make_attribute!(
        CKA_UNIQUE_ID,
        CString::new("2").unwrap().into_raw(),
        1
    )];
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    /* get values */
    let mut hash: [u8; 32] = [0; 32];
    let mut value: [u8; 32] = [0; 32];
    template.clear();
    template.push(make_attribute!(CKA_VALUE, value.as_mut_ptr(), value.len()));
    template.push(make_attribute!(
        CKA_OBJECT_ID,
        hash.as_mut_ptr(),
        hash.len()
    ));
    ret = fn_get_attribute_value(
        session,
        handle,
        template.as_mut_ptr(),
        template.len() as u64,
    );
    assert_eq!(ret, CKR_OK);

    let value_len = template[0].ulValueLen;

    /* one shot digest */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_SHA256,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    ret = fn_digest_init(session, &mut mechanism);
    assert_eq!(ret, CKR_OK);

    let mut digest: [u8; 32] = [0; 32];
    let mut digest_len: CK_ULONG = digest.len() as CK_ULONG;
    ret = fn_digest(
        session,
        value.as_mut_ptr(),
        value_len,
        digest.as_mut_ptr(),
        &mut digest_len,
    );
    assert_eq!(ret, CKR_OK);
    assert_eq!(hash, digest);

    /* update digest */
    ret = fn_digest_init(session, &mut mechanism);
    assert_eq!(ret, CKR_OK);

    ret = fn_digest_update(session, value.as_mut_ptr(), value_len);
    assert_eq!(ret, CKR_OK);

    let mut digest2_len: CK_ULONG = 0;
    ret = fn_digest_final(session, std::ptr::null_mut(), &mut digest2_len);
    assert_eq!(ret, CKR_OK);
    assert_eq!(digest_len, digest2_len);

    let mut digest2: [u8; 32] = [0; 32];
    ret = fn_digest_final(session, digest2.as_mut_ptr(), &mut digest2_len);
    assert_eq!(ret, CKR_OK);
    assert_eq!(hash, digest);

    /* ==== SHA 384 ==== */

    /* get test data */
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;
    let mut template = vec![make_attribute!(
        CKA_UNIQUE_ID,
        CString::new("3").unwrap().into_raw(),
        1
    )];
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    /* get values */
    let mut hash: [u8; 48] = [0; 48];
    let mut value: [u8; 48] = [0; 48];
    template.clear();
    template.push(make_attribute!(CKA_VALUE, value.as_mut_ptr(), value.len()));
    template.push(make_attribute!(
        CKA_OBJECT_ID,
        hash.as_mut_ptr(),
        hash.len()
    ));
    ret = fn_get_attribute_value(
        session,
        handle,
        template.as_mut_ptr(),
        template.len() as u64,
    );
    assert_eq!(ret, CKR_OK);

    let value_len = template[0].ulValueLen;

    /* one shot digest */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_SHA384,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    ret = fn_digest_init(session, &mut mechanism);
    assert_eq!(ret, CKR_OK);

    let mut digest: [u8; 48] = [0; 48];
    let mut digest_len: CK_ULONG = digest.len() as CK_ULONG;
    ret = fn_digest(
        session,
        value.as_mut_ptr(),
        value_len,
        digest.as_mut_ptr(),
        &mut digest_len,
    );
    assert_eq!(ret, CKR_OK);
    assert_eq!(hash, digest);

    /* ==== SHA 512 ==== */

    /* get test data */
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;
    let mut template = vec![make_attribute!(
        CKA_UNIQUE_ID,
        CString::new("4").unwrap().into_raw(),
        1
    )];
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    /* get values */
    let mut hash: [u8; 64] = [0; 64];
    let mut value: [u8; 64] = [0; 64];
    template.clear();
    template.push(make_attribute!(CKA_VALUE, value.as_mut_ptr(), value.len()));
    template.push(make_attribute!(
        CKA_OBJECT_ID,
        hash.as_mut_ptr(),
        hash.len()
    ));
    ret = fn_get_attribute_value(
        session,
        handle,
        template.as_mut_ptr(),
        template.len() as u64,
    );
    assert_eq!(ret, CKR_OK);

    let value_len = template[0].ulValueLen;

    /* one shot digest */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_SHA512,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    ret = fn_digest_init(session, &mut mechanism);
    assert_eq!(ret, CKR_OK);

    let mut digest: [u8; 64] = [0; 64];
    let mut digest_len: CK_ULONG = digest.len() as CK_ULONG;
    ret = fn_digest(
        session,
        value.as_mut_ptr(),
        value_len,
        digest.as_mut_ptr(),
        &mut digest_len,
    );
    assert_eq!(ret, CKR_OK);
    assert_eq!(hash, digest);

    /* ==== SHA 1 ==== */

    /* get test data */
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;
    let mut template = vec![make_attribute!(
        CKA_UNIQUE_ID,
        CString::new("5").unwrap().into_raw(),
        1
    )];
    ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    /* get values */
    let mut hash: [u8; 20] = [0; 20];
    let mut value: [u8; 20] = [0; 20];
    template.clear();
    template.push(make_attribute!(CKA_VALUE, value.as_mut_ptr(), value.len()));
    template.push(make_attribute!(
        CKA_OBJECT_ID,
        hash.as_mut_ptr(),
        hash.len()
    ));
    ret = fn_get_attribute_value(
        session,
        handle,
        template.as_mut_ptr(),
        template.len() as u64,
    );
    assert_eq!(ret, CKR_OK);

    let value_len = template[0].ulValueLen;

    /* one shot digest */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_SHA_1,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    ret = fn_digest_init(session, &mut mechanism);
    assert_eq!(ret, CKR_OK);

    let mut digest: [u8; 20] = [0; 20];
    let mut digest_len: CK_ULONG = digest.len() as CK_ULONG;
    ret = fn_digest(
        session,
        value.as_mut_ptr(),
        value_len,
        digest.as_mut_ptr(),
        &mut digest_len,
    );
    assert_eq!(ret, CKR_OK);
    assert_eq!(hash, digest);

    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

struct TestCase {
    value: Vec<u8>,
    result: Vec<u8>,
}

/* name in CKA_APPLICATION, value in CKA_VALUE, result in CKA_OBJECT_ID */
fn get_test_case_data(session: CK_SESSION_HANDLE, name: &str) -> TestCase {
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;
    let mut template = vec![make_attribute!(
        CKA_APPLICATION,
        CString::new(name).unwrap().into_raw(),
        name.len()
    )];
    let mut ret = fn_find_objects_init(session, template.as_mut_ptr(), 1);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    /* get values */
    template.clear();
    template.push(make_attribute!(CKA_VALUE, std::ptr::null_mut(), 0));
    template.push(make_attribute!(CKA_OBJECT_ID, std::ptr::null_mut(), 0));
    ret = fn_get_attribute_value(
        session,
        handle,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);
    let mut tc = TestCase {
        value: vec![0; template[0].ulValueLen as usize],
        result: vec![0; template[1].ulValueLen as usize],
    };
    template[0].pValue = tc.value.as_mut_ptr() as CK_VOID_PTR;
    template[1].pValue = tc.result.as_mut_ptr() as CK_VOID_PTR;
    ret = fn_get_attribute_value(
        session,
        handle,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    tc
}

/* name in CKA_ID */
fn get_test_key_handle(
    session: CK_SESSION_HANDLE,
    name: &str,
    class: CK_ULONG,
) -> CK_OBJECT_HANDLE {
    let mut handle: CK_ULONG = CK_INVALID_HANDLE;
    let mut classbuf = class;
    let mut template = vec![
        make_attribute!(
            CKA_ID,
            CString::new(name).unwrap().into_raw(),
            name.len()
        ),
        make_attribute!(CKA_CLASS, &mut classbuf as *mut _, CK_ULONG_SIZE),
    ];
    let mut ret = fn_find_objects_init(session, template.as_mut_ptr(), 2);
    assert_eq!(ret, CKR_OK);
    let mut count: CK_ULONG = 0;
    ret = fn_find_objects(session, &mut handle, 1, &mut count);
    assert_eq!(ret, CKR_OK);
    assert_eq!(count, 1);
    ret = fn_find_objects_final(session);
    assert_eq!(ret, CKR_OK);

    handle
}

fn sig_verify(
    session: CK_SESSION_HANDLE,
    key: CK_OBJECT_HANDLE,
    data: &mut Vec<u8>,
    signature: &mut Vec<u8>,
    mechanism: &mut CK_MECHANISM,
) {
    let mut ret = fn_verify_init(session, mechanism, key);
    assert_eq!(ret, CKR_OK);

    ret = fn_verify(
        session,
        data.as_mut_ptr(),
        data.len() as CK_ULONG,
        signature.as_mut_ptr(),
        signature.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);
}

fn sig_and_check(
    session: CK_SESSION_HANDLE,
    key: CK_OBJECT_HANDLE,
    data: &mut Vec<u8>,
    signature: &mut Vec<u8>,
    mechanism: &mut CK_MECHANISM,
) -> Vec<u8> {
    let mut ret = fn_sign_init(session, mechanism, key);
    assert_eq!(ret, CKR_OK);

    /* get signature lenght */
    let mut csiglen: CK_ULONG = 0;
    ret = fn_sign(
        session,
        data.as_mut_ptr(),
        data.len() as CK_ULONG,
        std::ptr::null_mut(),
        &mut csiglen,
    );
    assert_eq!(ret, CKR_OK);
    assert_eq!(csiglen, signature.len() as CK_ULONG);

    let mut computed_signature: Vec<u8> = vec![0; csiglen as usize];
    ret = fn_sign(
        session,
        data.as_mut_ptr(),
        data.len() as CK_ULONG,
        computed_signature.as_mut_ptr(),
        &mut csiglen,
    );
    assert_eq!(ret, CKR_OK);
    assert_eq!(csiglen, signature.len() as CK_ULONG);
    assert_eq!(signature, &mut computed_signature);

    computed_signature
}

#[test]
fn test_signatures() {
    /* Test Vectors from python cryptography's pkcs1v15sign-vectors.txt */
    let mut testdata = TestData::new("testdata/test_sign_verify.json");

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);

    /* open session */
    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    /* login */
    let pin = "12345678";
    ret = fn_login(
        session,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* ### CKM_RSA_PKCS ### */

    /* get test data */
    let mut testcase = get_test_case_data(session, "CKM_RSA_PKCS");
    let pri_key_handle =
        get_test_key_handle(session, "Example 15", CKO_PRIVATE_KEY);
    let pub_key_handle =
        get_test_key_handle(session, "Example 15", CKO_PUBLIC_KEY);

    /* verify test vector */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_SHA1_RSA_PKCS,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    sig_verify(
        session,
        pub_key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );

    let _ = sig_and_check(
        session,
        pri_key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );

    /* ### HMACs ### */

    /* get test keys */
    let key_handle =
        get_test_key_handle(session, "HMAC Test Key", CKO_SECRET_KEY);

    /* ### SHA-1 HMAC */

    /* get test data */
    let mut testcase = get_test_case_data(session, "CKM_SHA_1_HMAC");

    /* verify test vector */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_SHA_1_HMAC,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    sig_verify(
        session,
        key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );
    let _ = sig_and_check(
        session,
        key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );

    /* ### SHA256 HMAC */

    /* get test data */
    let mut testcase = get_test_case_data(session, "CKM_SHA256_HMAC");

    /* verify test vector */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_SHA256_HMAC,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    sig_verify(
        session,
        key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );
    let _ = sig_and_check(
        session,
        key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );

    /* ### SHA384 HMAC */

    /* get test data */
    let mut testcase = get_test_case_data(session, "CKM_SHA384_HMAC");

    /* verify test vector */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_SHA384_HMAC,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    sig_verify(
        session,
        key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );
    let _ = sig_and_check(
        session,
        key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );

    /* ### SHA512 HMAC */

    /* get test data */
    let mut testcase = get_test_case_data(session, "CKM_SHA512_HMAC");

    /* verify test vector */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_SHA512_HMAC,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    sig_verify(
        session,
        key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );
    let _ = sig_and_check(
        session,
        key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );

    /* ### SHA3 256 HMAC ### */

    /* get test keys */
    let key_handle =
        get_test_key_handle(session, "HMAC SHA-3-256 Test Key", CKO_SECRET_KEY);

    /* get test data */
    let mut testcase = get_test_case_data(session, "CKM_SHA3_256_HMAC");

    /* verify test vector */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_SHA3_256_HMAC,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };
    sig_verify(
        session,
        key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );
    let _ = sig_and_check(
        session,
        key_handle,
        &mut testcase.value,
        &mut testcase.result,
        &mut mechanism,
    );

    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}

#[test]
fn test_keygen() {
    let mut testdata = TestData::new("testdata/test_keygen.json");
    testdata.setup_db();

    let mut args = testdata.make_init_args();
    let args_ptr = &mut args as *mut CK_C_INITIALIZE_ARGS;
    let mut ret = fn_initialize(args_ptr as *mut std::ffi::c_void);
    assert_eq!(ret, CKR_OK);
    let mut session: CK_SESSION_HANDLE = CK_UNAVAILABLE_INFORMATION;
    ret = fn_open_session(
        testdata.get_slot(),
        CKF_SERIAL_SESSION | CKF_RW_SESSION,
        std::ptr::null_mut(),
        None,
        &mut session,
    );
    assert_eq!(ret, CKR_OK);

    /* login */
    let pin = "12345678";
    ret = fn_login(
        session,
        CKU_USER,
        pin.as_ptr() as *mut _,
        pin.len() as CK_ULONG,
    );
    assert_eq!(ret, CKR_OK);

    /* Generic Secret */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_GENERIC_SECRET_KEY_GEN,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };

    let mut handle: CK_ULONG = CK_INVALID_HANDLE;

    let mut class = CKO_SECRET_KEY;
    let mut len: CK_ULONG = 64;
    let mut template = vec![
        make_attribute!(CKA_CLASS, &mut class as *mut _, CK_ULONG_SIZE),
        make_attribute!(CKA_VALUE_LEN, &mut len as *mut _, CK_ULONG_SIZE),
    ];

    ret = fn_generate_key(
        session,
        &mut mechanism,
        template.as_mut_ptr(),
        template.len() as CK_ULONG,
        &mut handle,
    );
    assert_eq!(ret, CKR_OK);

    /* RSA key pair */
    let mut mechanism: CK_MECHANISM = CK_MECHANISM {
        mechanism: CKM_RSA_PKCS_KEY_PAIR_GEN,
        pParameter: std::ptr::null_mut(),
        ulParameterLen: 0,
    };

    let mut pubkey = CK_INVALID_HANDLE;
    let mut prikey = CK_INVALID_HANDLE;

    let mut truebool = CK_TRUE;
    let mut len: CK_ULONG = 2048;
    let mut pub_template = vec![
        make_attribute!(CKA_ENCRYPT, &mut truebool as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_VERIFY, &mut truebool as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_MODULUS_BITS, &mut len as *mut _, CK_ULONG_SIZE),
    ];
    let mut pri_template = vec![
        make_attribute!(CKA_PRIVATE, &mut truebool as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_SENSITIVE, &mut truebool as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_TOKEN, &mut truebool as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_DECRYPT, &mut truebool as *mut _, CK_BBOOL_SIZE),
        make_attribute!(CKA_SIGN, &mut truebool as *mut _, CK_BBOOL_SIZE),
    ];

    ret = fn_generate_key_pair(
        session,
        &mut mechanism,
        pub_template.as_mut_ptr(),
        pub_template.len() as CK_ULONG,
        pri_template.as_mut_ptr(),
        pri_template.len() as CK_ULONG,
        &mut pubkey,
        &mut prikey,
    );
    assert_eq!(ret, CKR_OK);

    ret = fn_close_session(session);
    assert_eq!(ret, CKR_OK);

    testdata.finalize();
}
