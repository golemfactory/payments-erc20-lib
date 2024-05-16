/// Check if error is in a known list of common RPC problems
pub fn check_if_proper_rpc_error(err: &str) -> bool {
    if err.contains("transfer amount exceeds balance") {
        return true;
    }
    if err.contains("already known") {
        return true;
    }
    if err.contains("insufficient funds") {
        return true;
    }
    if err.contains("nonce too low") {
        return true;
    }
    false
}
