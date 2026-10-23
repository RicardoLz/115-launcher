package main

import (
	"fmt"
	"strings"
)

func hx(s string) (int, bool) {
	v := 0
	for _, c := range s {
		v <<= 4
		switch {
		case c >= '0' && c <= '9':
			v += int(c - '0')
		case c >= 'a' && c <= 'f':
			v += int(c-'a') + 10
		case c >= 'A' && c <= 'F':
			v += int(c-'A') + 10
		default:
			return 0, false
		}
	}
	return v, true
}

// 专查：合法 GBK 双字节对上 go_gbk 与 rs_gbk 是否逐字节相等
func auditValidPairs() {
	g := readCSV("go_out.csv", []int{1, 2})
	r := readCSV("rs_out.csv", []int{1, 2, 3, 4})
	valid, diffs := 0, 0
	var first []string
	replCountDiff := 0
	for k, gv := range g {
		rv, has := r[k]
		if !has {
			continue
		}
		if len(k) == 4 {
			lead, _ := hx(k[:2])
			trail, _ := hx(k[2:])
			ok := (lead == 0x80) ||
				(lead >= 0x81 && lead <= 0xFE && ((trail >= 0x40 && trail <= 0x7E) || (trail >= 0x80 && trail <= 0xFE)))
			if ok {
				valid++
				if gv[1] != rv[0] {
					diffs++
					if len(first) < 15 {
						first = append(first, fmt.Sprintf("in=%s go=%s rs=%s", k, gv[1], rv[0]))
					}
				}
			}
		}
		if len(k) != 8 && gv[1] != rv[0] &&
			strings.Count(rv[0], "EFBFBD") != strings.Count(gv[1], "EFBFBD") {
			replCountDiff++
		}
	}
	fmt.Printf("\n== 合法GBK双字节对(含0x80): %d，go_gbk≠rs_gbk: %d\n", valid, diffs)
	for _, s := range first {
		fmt.Println("  " + s)
	}
	fmt.Printf("非4字节输入中替换符个数不同的行数: %d\n", replCountDiff)
}

// 差异是否全部落在 GBK 保留用户造字区
func auditReserved() {
	g := readCSV("go_out.csv", []int{1, 2})
	r := readCSV("rs_out.csv", []int{1, 2, 3, 4})
	reserved := func(lead, trail int) bool {
		inLead := func(lo, hi int) bool { return lead >= lo && lead <= hi }
		// x/text GBK 表未收录的保留区（GBK 2000 用户自定义）
		return lead == 0xA1 || lead == 0xA2 || lead == 0xA3 || inLead(0xA4, 0xAF) || inLead(0xF8, 0xFE) || false; // x || inLead(0xA6, 0xAF) || inLead(0xF8, 0xFE)
	}
	total, inReserved, pua := 0, 0, 0
	var outside []string
	for k, gv := range g {
		rv, has := r[k]
		if !has || len(k) != 4 || gv[1] == rv[0] {
			continue
		}
		lead, _ := hx(k[:2])
		trail, _ := hx(k[2:])
		ok := lead >= 0x81 && lead <= 0xFE && ((trail >= 0x40 && trail <= 0x7E) || (trail >= 0x80 && trail <= 0xFE))
		if !ok {
			continue
		}
		total++
		if reserved(lead, trail) {
			inReserved++
			if strings.HasPrefix(rv[0], "EE") || strings.HasPrefix(rv[0], "EF") {
				pua++
			}
		} else if len(outside) < 10 {
			outside = append(outside, fmt.Sprintf("in=%s go=%s rs=%s", k, gv[1], rv[0]))
		}
	}
	fmt.Printf("\n== 双字节差异 %d 条，其中落在保留用户造字区: %d（映射到 PUA: %d），区外差异: %d\n",
		total, inReserved, pua, total-inReserved)
	for _, s := range outside {
		fmt.Println("  区外: " + s)
	}
}
