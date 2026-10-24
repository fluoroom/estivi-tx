//! International Morse code table, shared by the CW identifier (TX) and the CW decoder (RX).

const TABLE: &[(char, &str)] = &[
    ('A', ".-"),
    ('B', "-..."),
    ('C', "-.-."),
    ('D', "-.."),
    ('E', "."),
    ('F', "..-."),
    ('G', "--."),
    ('H', "...."),
    ('I', ".."),
    ('J', ".---"),
    ('K', "-.-"),
    ('L', ".-.."),
    ('M', "--"),
    ('N', "-."),
    ('O', "---"),
    ('P', ".--."),
    ('Q', "--.-"),
    ('R', ".-."),
    ('S', "..."),
    ('T', "-"),
    ('U', "..-"),
    ('V', "...-"),
    ('W', ".--"),
    ('X', "-..-"),
    ('Y', "-.--"),
    ('Z', "--.."),
    ('0', "-----"),
    ('1', ".----"),
    ('2', "..---"),
    ('3', "...--"),
    ('4', "....-"),
    ('5', "....."),
    ('6', "-...."),
    ('7', "--..."),
    ('8', "---.."),
    ('9', "----."),
    ('/', "-..-."),
    ('?', "..--.."),
    ('.', ".-.-.-"),
    (',', "--..--"),
    ('=', "-...-"),
    ('-', "-....-"),
    ('+', ".-.-."),
    ('@', ".--.-."),
    (':', "---..."),
    ('\'', ".----."),
    ('(', "-.--."),
    (')', "-.--.-"),
    ('!', "-.-.--"),
    ('"', ".-..-."),
];

pub fn encode(c: char) -> Option<&'static str> {
    let c = c.to_ascii_uppercase();
    TABLE.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}

pub fn decode(code: &str) -> Option<char> {
    TABLE.iter().find(|(_, v)| *v == code).map(|(k, _)| *k)
}

/// Length of one Morse unit (a dit) in milliseconds, PARIS standard.
pub fn unit_ms(wpm: f32) -> f64 {
    1200.0 / wpm.max(1.0) as f64
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        for c in "ABCXYZ0189/?".chars() {
            assert_eq!(super::decode(super::encode(c).unwrap()), Some(c));
        }
    }
}
