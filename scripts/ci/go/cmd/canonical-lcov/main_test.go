package main

import (
	"math"
	"reflect"
	"testing"
)

func TestCanonicalPath(t *testing.T) {
	top := "/repo"
	cases := []struct {
		name string
		sf   string
		want string
		ok   bool
	}{
		{"absolute in tree", "/repo/rust/src/main.rs", "/repo/rust/src/main.rs", true},
		{"relative joins top", "rust/src/main.rs", "/repo/rust/src/main.rs", true},
		{"twin spelling merges", "/repo/rust/tests/../src/main.rs", "/repo/rust/src/main.rs", true},
		{"dot segments clean", "/repo/rust/./src/main.rs", "/repo/rust/src/main.rs", true},
		{"outside tree rejected", "/other/src/main.rs", "", false},
		{"top itself rejected", "/repo", "", false},
		{"tests dir excluded", "/repo/rust/tests/foo.rs", "", false},
		{"target dir excluded", "/repo/rust/target/debug/x.rs", "", false},
		{"cargo registry excluded", "/repo/.cargo/registry/x.rs", "", false},
		{"rustup excluded", "/repo/rustup/toolchains/x.rs", "", false},
		{"src file kept", "/repo/rust/src/diff.rs", "/repo/rust/src/diff.rs", true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got, ok := canonicalPath(top, tc.sf)
			if got != tc.want || ok != tc.ok {
				t.Fatalf("canonicalPath(%q) = (%q, %v), want (%q, %v)", tc.sf, got, ok, tc.want, tc.ok)
			}
		})
	}
}

func TestAddCount(t *testing.T) {
	t.Run("sums hits", func(t *testing.T) {
		record := map[int]int64{10: 3}
		addCount(record, 10, 4)
		addCount(record, 11, 1)
		if !reflect.DeepEqual(record, map[int]int64{10: 7, 11: 1}) {
			t.Fatalf("record = %v", record)
		}
	})
	t.Run("clamp marker reads as zero but records the line", func(t *testing.T) {
		record := map[int]int64{}
		addCount(record, 5, clamp)
		addCount(record, 6, clamp+100)
		if !reflect.DeepEqual(record, map[int]int64{5: 0, 6: 0}) {
			t.Fatalf("record = %v", record)
		}
	})
	t.Run("below clamp adds", func(t *testing.T) {
		record := map[int]int64{}
		addCount(record, 5, clamp-1)
		if record[5] != int64(clamp-1) {
			t.Fatalf("record[5] = %d", record[5])
		}
	})
	t.Run("saturates at maxint64", func(t *testing.T) {
		record := map[int]int64{7: math.MaxInt64 - 10}
		addCount(record, 7, 100)
		if record[7] != math.MaxInt64 {
			t.Fatalf("record[7] = %d", record[7])
		}
	})
	t.Run("exact max does not saturate early", func(t *testing.T) {
		record := map[int]int64{7: math.MaxInt64 - 10}
		addCount(record, 7, 10)
		if record[7] != math.MaxInt64 {
			t.Fatalf("record[7] = %d", record[7])
		}
	})
}

func TestParseCanonical(t *testing.T) {
	raw := "SF:/repo/rust/src/a.rs\n" +
		"DA:1,3\n" +
		"DA:2,0\n" +
		"end_of_record\n" +
		"SF:/repo/rust/tests/../src/a.rs\n" +
		"DA:1,4\n" +
		"DA:3,1\n" +
		"end_of_record\n" +
		"SF:/repo/rust/tests/b.rs\n" +
		"DA:1,9\n" +
		"end_of_record\n" +
		"SF:/elsewhere/c.rs\n" +
		"DA:1,9\n" +
		"end_of_record\n" +
		"SF:relative/d.rs\n" +
		"DA:2,2\n" +
		"end_of_record\n"
	got := parseCanonical(raw, "/repo")
	want := map[string]map[int]int64{
		"/repo/rust/src/a.rs": {1: 7, 2: 0, 3: 1},
		"/repo/relative/d.rs": {2: 2},
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("parseCanonical = %v, want %v", got, want)
	}
}

func TestParseCanonicalSkipsMalformedDA(t *testing.T) {
	raw := "SF:/repo/rust/src/a.rs\n" +
		"DA:1\n" +
		"DA:x,y\n" +
		"DA:2,2\n" +
		"end_of_record\n"
	got := parseCanonical(raw, "/repo")
	want := map[string]map[int]int64{"/repo/rust/src/a.rs": {2: 2}}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("parseCanonical = %v, want %v", got, want)
	}
}
