package main

import (
	"fmt"
	"strings"
)

func main() {
	rs := load("pvf_rs.csv", 0, 1, 2)
	goT := load("pvf_go.csv", 0, 2, 1)
	hasFFFD := func(h string) bool { return strings.Contains(h, "EFBFBD") }
	hasPUA := func(h string) bool {
		return strings.Contains(h, "EE") || strings.Contains(h, "EFBC") || strings.Contains(h, "F1") ||
			strings.Contains(h, "F2") || strings.Contains(h, "F3")
	}
	utf16ish := func(in string) bool {
		if len(in) < 8 {
			return false
		}
		n := 0
		for i := 0; i+4 <= len(in); i += 4 {
			if in[i:i+4] == "0000" || (in[i:i+2] == "00") != (in[i+2:i+4] == "00") {
				n++
			}
		}
		return n > (len(in)/4)/2
	}
	cats := map[string]int{}
	var ex [][3]string
	for k, gv := range goT {
		rv, ok := rs[k]
		if !ok || gv.gbk == rv.gbk {
			continue
		}
		c := "纯GBK文本却不同"
		switch {
		case utf16ish(k):
			c = "UTF16混杂行"
		case hasPUA(gv.gbk) || hasPUA(rv.gbk):
			c = "含PUA映射"
		case hasFFFD(gv.gbk) || hasFFFD(rv.gbk):
			c = "含替换符"
		}
		cats[c]++
		if c == "纯GBK文本却不同" && len(ex) < 6 {
			ex = append(ex, [3]string{k, gv.gbk, rv.gbk})
		}
	}
	for k, v := range cats {
		fmt.Printf("%s: %d\n", k, v)
	}
	for _, e := range ex {
		fmt.Printf("  in=%s\ngo=%s\nrs=%s\n", e[0], e[1], e[2])
	}
	// 再全量统计三类行上的一致性
	var n16, n16diff, nPure, nPurediff int
	for k, gv := range goT {
		rv, ok := rs[k]
		if !ok {
			continue
		}
		if utf16ish(k) {
			n16++
			if gv.gbk != rv.gbk {
				n16diff++
			}
		} else if !hasPUA(gv.gbk) && !hasPUA(rv.gbk) && !hasFFFD(gv.gbk) && !hasFFFD(rv.gbk) {
			nPure++
			if gv.gbk != rv.gbk {
				nPurediff++
				if nPurediff <= 5 {
					fmt.Printf("  纯文本差异 in=%s go=%s rs=%s\n", k, gv.gbk, rv.gbk)
				}
			}
		}
	}
	fmt.Printf("\nUTF16混杂行: %d（不同 %d）；纯正常文本行: %d（不同 %d）\n", n16, n16diff, nPure, nPurediff)
}

func load(path string, keyCol, gbkCol, g18Col int) map[string]pair { return nil }
