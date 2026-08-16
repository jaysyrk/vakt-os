package main

import "testing"

func TestParseScanSortsByStrength(t *testing.T) {
	got := parseScan("weak\t-80\t2437\tWPA2\nstrong\t-40\t2437\tWPA2\nmiddle\t-60\t2437\tWPA3\n")

	want := []string{"strong", "middle", "weak"}
	if len(got) != len(want) {
		t.Fatalf("got %d networks, want %d", len(got), len(want))
	}
	for i, ssid := range want {
		if got[i].SSID != ssid {
			t.Errorf("position %d is %q, want %q", i, got[i].SSID, ssid)
		}
	}
}

// A dual-band router advertises the same SSID twice. Listing it twice invites
// picking the weaker radio for no reason.
func TestParseScanCollapsesBothBands(t *testing.T) {
	got := parseScan("Stykezone\t-70\t2437\tWPA2\nStykezone\t-45\t5180\tWPA2\n")

	if len(got) != 1 {
		t.Fatalf("got %d networks, want 1", len(got))
	}
	if got[0].Signal != -45 || got[0].Freq != 5180 {
		t.Errorf("kept %d dBm at %d MHz, want the stronger -45 at 5180", got[0].Signal, got[0].Freq)
	}
}

func TestBandFromFrequency(t *testing.T) {
	cases := []struct {
		freq int
		want string
	}{
		{2412, "2.4GHz"},
		{2484, "2.4GHz"},
		{5180, "5GHz"},
		{5825, "5GHz"},
	}
	for _, c := range cases {
		if got := (network{Freq: c.freq}).band(); got != c.want {
			t.Errorf("%d MHz is %q, want %q", c.freq, got, c.want)
		}
	}
}

func TestBarsAlwaysOccupyFourCells(t *testing.T) {
	for signal := -120; signal <= 0; signal += 5 {
		bars := []rune((network{Signal: signal}).bars())
		if len(bars) != 4 {
			t.Fatalf("%d dBm rendered %d cells, want 4", signal, len(bars))
		}
	}
}

func TestParseScanSkipsMalformedLines(t *testing.T) {
	got := parseScan("\t-40\t2437\tWPA2\nnofields\ngood\t-50\t2437\tWPA2\nbad\tnotanumber\t2437\tWPA2\nbad\t-50\tnotanumber\tWPA2\n")

	if len(got) != 1 {
		t.Fatalf("got %d networks, want only the well-formed one: %+v", len(got), got)
	}
	if got[0].SSID != "good" {
		t.Errorf("kept %q, want %q", got[0].SSID, "good")
	}
}
