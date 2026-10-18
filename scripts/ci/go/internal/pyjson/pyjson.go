// Package pyjson mirrors Python json.dumps(obj, indent=1): 1-space
// indent, no HTML escaping, non-ASCII as \uXXXX (surrogate pairs
// above the BMP, like ensure_ascii=True), plus the caller's trailing
// newline. (One known gap: Python short-escapes \b and \f while Go
// emits \u0008/\u000c; neither appears in tool baselines.)
package pyjson

import (
	"bytes"
	"encoding/json"
	"unicode/utf8"
)

// MarshalIndent1 renders v like json.dumps(v, indent=1) + "\n".
func MarshalIndent1(v interface{}) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", " ")
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return asciiEscape(buf.Bytes()), nil
}

const hexd = "0123456789abcdef"

// asciiEscape rewrites every non-ASCII rune as \uXXXX (lowercase hex,
// surrogate pairs above the BMP). Valid encoder output holds
// non-ASCII only inside string literals, so a byte scan suffices.
func asciiEscape(in []byte) []byte {
	i := 0
	for i < len(in) && in[i] < 0x80 {
		i++
	}
	if i == len(in) {
		return in
	}
	out := make([]byte, 0, len(in)+16)
	out = append(out, in[:i]...)
	for i < len(in) {
		c := in[i]
		if c < 0x80 {
			out = append(out, c)
			i++
			continue
		}
		r, w := utf8.DecodeRune(in[i:])
		if r == utf8.RuneError && w == 1 {
			r = 0xFFFD
		}
		if r > 0xFFFF {
			r -= 0x10000
			appendEscape(&out, 0xD800+r>>10)
			appendEscape(&out, 0xDC00+r&0x3FF)
		} else {
			appendEscape(&out, r)
		}
		i += w
	}
	return out
}

func appendEscape(out *[]byte, r rune) {
	*out = append(*out, '\\', 'u',
		hexd[byte(r>>12)&0xF], hexd[byte(r>>8)&0xF],
		hexd[byte(r>>4)&0xF], hexd[byte(r)&0xF])
}
