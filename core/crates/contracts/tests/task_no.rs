use aite_contracts::{TaskNoError, encode_task_no};

#[test]
fn four_vectors() {
    assert_eq!(encode_task_no(1).unwrap(), "#A1");
    assert_eq!(encode_task_no(17).unwrap(), "#AH");
    assert_eq!(encode_task_no(32).unwrap(), "#A10");
    assert_eq!(encode_task_no(1000).unwrap(), "#AZ8");
}

#[test]
fn zero_is_rejected() {
    assert_eq!(encode_task_no(0), Err(TaskNoError(0)));
}

#[test]
fn crockford_alphabet_skips_i_l_o_u() {
    for n in 1..2000u64 {
        let s = encode_task_no(n).unwrap();
        assert!(s.starts_with("#A"));
        assert!(!s[2..].chars().any(|c| "ILOU".contains(c)), "{s}");
    }
}
