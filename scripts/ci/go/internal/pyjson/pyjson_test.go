package pyjson

import (
	"encoding/json"
	"testing"
)

// Expected bytes generated once from CPython json.dumps(indent=1).
func TestMatchesCPython(t *testing.T) {
	v := map[string]interface{}{
		"angle":  "x>y<z&w",
		"quote":  "say \"hi\" \\ bye",
		"nl":     "a\nb\tc\rd",
		"latin":  "café",
		"astral": "𝄞",
		"ctrl":   "\u0001",
		"num":    json.Number("30"),
		"int":    7,
		"list":   []interface{}{"él", json.Number("1.5")},
	}
	got, err := MarshalIndent1(v)
	if err != nil {
		t.Fatal(err)
	}
	want := "{\n" +
		" \"angle\": \"x>y<z&w\",\n" +
		" \"astral\": \"\\ud834\\udd1e\",\n" +
		" \"ctrl\": \"\\u0001\",\n" +
		" \"int\": 7,\n" +
		" \"latin\": \"caf\\u00e9\",\n" +
		" \"list\": [\n" +
		"  \"\\u00e9l\",\n" +
		"  1.5\n" +
		" ],\n" +
		" \"nl\": \"a\\nb\\tc\\rd\",\n" +
		" \"num\": 30,\n" +
		" \"quote\": \"say \\\"hi\\\" \\\\ bye\"\n" +
		"}\n"
	if string(got) != want {
		t.Fatalf("mismatch:\n got: %q\nwant: %q", got, want)
	}
}

func TestASCIIFastPath(t *testing.T) {
	got, err := MarshalIndent1(map[string]string{"a": "b"})
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != "{\n \"a\": \"b\"\n}\n" {
		t.Fatalf("got %q", got)
	}
}
