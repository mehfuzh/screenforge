//! Screen Studio ids are 10-character nanoid strings over `[A-Za-z0-9]`
//! (e.g. `VTxKFiwiin`, `9AxbV2Fghr`).

const ALPHABET: [char; 62] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I',
    'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', 'a', 'b',
    'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u',
    'v', 'w', 'x', 'y', 'z',
];

pub const ID_LEN: usize = 10;

/// Generate a new Screen Studio style id.
pub fn new_id() -> String {
    nanoid::nanoid!(ID_LEN, &ALPHABET)
}

#[cfg(test)]
mod tests {
    #[test]
    fn id_shape() {
        let id = super::new_id();
        assert_eq!(id.len(), 10);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
