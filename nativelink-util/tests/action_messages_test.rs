use hex::FromHex;
use nativelink_macro::nativelink_test;
use nativelink_util::action_messages::{ActionInfo, ActionUniqueKey, ActionUniqueQualifier};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;

fn make_key() -> ActionUniqueKey {
    ActionUniqueKey {
        instance_name: String::from("main"),
        digest_function: DigestHasherFunc::Sha256,
        digest: DigestInfo::new(
            <[u8; 32]>::from_hex(
                "38fc5a8a97c1217160b0b658751979b9a1171286381489c8b98c993ec40b1546",
            )
            .unwrap(),
            210,
        ),
    }
}

#[nativelink_test]
fn old_unique_qualifier_cachable_works() {
    let contents = include_str!("data/action_message_cachable_060.json");
    let action_info: ActionInfo = serde_json::from_str(contents).unwrap();
    assert_eq!(
        action_info.unique_qualifier,
        ActionUniqueQualifier::Cacheable(make_key())
    );
}

#[nativelink_test]
fn old_unique_qualifier_uncachable_works() {
    let contents = include_str!("data/action_message_uncachable_060.json");
    let action_info: ActionInfo = serde_json::from_str(contents).unwrap();
    assert_eq!(
        action_info.unique_qualifier,
        ActionUniqueQualifier::Uncacheable(make_key())
    );
}
