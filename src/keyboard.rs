//! Cardputer Adv のキーボード (TCA8418) 用のキーマップ変換。
//!
//! TCA8418 が返す生の (row, col) を Cardputer の物理配列 (row, col) に
//! 変換し、文字へマッピングする純粋関数群。ペリフェラルには依存しない。

/// TCA8418 の生スキャン座標を Cardputer の物理配列 (row, col) に変換する。
pub fn remap_key(raw_row: u8, raw_col: u8) -> (u8, u8) {
    let mut col = raw_row * 2;

    if raw_col > 3 {
        col += 1;
    }

    let row = (raw_col + 4) % 4;

    (row, col)
}

/// Shift 押下時の文字変換。
pub fn shift_char(ch: char) -> char {
    match ch {
        'a'..='z' => ch.to_ascii_uppercase(),

        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',

        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '`' => '~',

        _ => ch,
    }
}

/// 生スキャン座標から通常（非 Shift）の文字を返す。修飾キー等は `None`。
pub fn key_to_char(raw_row: u8, raw_col: u8) -> Option<char> {
    let (row, col) = remap_key(raw_row, raw_col);

    match (row, col) {
        // Row 0
        (0, 0) => Some('`'),
        (0, 1) => Some('1'),
        (0, 2) => Some('2'),
        (0, 3) => Some('3'),
        (0, 4) => Some('4'),
        (0, 5) => Some('5'),
        (0, 6) => Some('6'),
        (0, 7) => Some('7'),
        (0, 8) => Some('8'),
        (0, 9) => Some('9'),
        (0, 10) => Some('0'),
        (0, 11) => Some('-'),
        (0, 12) => Some('='),

        // Row 1
        (1, 1) => Some('q'),
        (1, 2) => Some('w'),
        (1, 3) => Some('e'),
        (1, 4) => Some('r'),
        (1, 5) => Some('t'),
        (1, 6) => Some('y'),
        (1, 7) => Some('u'),
        (1, 8) => Some('i'),
        (1, 9) => Some('o'),
        (1, 10) => Some('p'),
        (1, 11) => Some('['),
        (1, 12) => Some(']'),
        (1, 13) => Some('\\'),

        // Row 2
        (2, 2) => Some('a'),
        (2, 3) => Some('s'),
        (2, 4) => Some('d'),
        (2, 5) => Some('f'),
        (2, 6) => Some('g'),
        (2, 7) => Some('h'),
        (2, 8) => Some('j'),
        (2, 9) => Some('k'),
        (2, 10) => Some('l'),
        (2, 11) => Some(';'),
        (2, 12) => Some('\''),

        // Row 3
        (3, 3) => Some('z'),
        (3, 4) => Some('x'),
        (3, 5) => Some('c'),
        (3, 6) => Some('v'),
        (3, 7) => Some('b'),
        (3, 8) => Some('n'),
        (3, 9) => Some('m'),
        (3, 10) => Some(','),
        (3, 11) => Some('.'),
        (3, 12) => Some('/'),
        (3, 13) => Some(' '),

        _ => None,
    }
}
