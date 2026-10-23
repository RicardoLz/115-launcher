// 合表：go_out.csv x rs_out.csv -> 差异分类报告
package main

import (
	"bufio"
	"encoding/csv"
	"fmt"
	"io"
	"os"
	"sort"
	"strings"
)

type row struct {
	in, g18, gbk, r18, rGBK string
}

func readCSV(path string, cols []int) map[string][]string {
	f, err := os.Open(path)
	if err != nil {
		panic(err)
	}
	defer f.Close()
	r := csv.NewReader(bufio.NewReaderSize(f, 1<<20))
	r.FieldsPerRecord = -1
	out := map[string][]string{}
	first := true
	for {
		rec, err := r.Read()
		if err == io.EOF {
			break
		}
		if err != nil {
			panic(fmt.Sprintf("%s: %v", path, err))
		}
		if first {
			first = false
			continue
		}
		key := strings.ToUpper(rec[0])
		var v []string
		for _, c := range cols {
			v = append(v, strings.ToUpper(rec[c]))
		}
		out[key] = v
		if err != nil {
			break
		}
	}
	return out
}

func classify(g18, gbk, r18, rGBK string) string {
	switch {
	case gbk == rGBK && g18 == r18:
		return "相同"
	case gbk == rGBK:
		return "仅GB18030列不同"
	case g18 == r18:
		return "仅GBK列不同"
	default:
		return "两列都不同"
	}
}

func cp(hexStr string) string {
	if len(hexStr) < 6 {
		return hexStr
	}
	// 只取第一个码点做摘要
	n := 2
	switch {
	case hexStr[0] >= 'F':
		n = 8
	case hexStr[0] >= 'E':
		n = 6
	case hexStr[0] >= 'C':
		n = 4
	}
	return hexStr[:n]
}

func main() {
	goTbl := readCSV("go_out.csv", []int{1, 2})
	rsTbl := readCSV("rs_out.csv", []int{1, 2, 3, 4})
	counts := map[string]int{}
	examples := map[string][]string{}
	var rows []row
	total := 0
	for k, gv := range goTbl {
		rv, ok := rsTbl[k]
		if !ok {
			continue
		}
		total++
		// rs_out 列序: rs_gbk_hex(0), rs_gb18030_hex(1), repl_gbk(2), repl_18(3)
		r := row{in: k, g18: gv[0], gbk: gv[1], r18: rv[1], rGBK: rv[0]}
		rows = append(rows, r)
		c := classify(r.g18, r.gbk, r.r18, r.rGBK)
		counts[c]++
		if c != "相同" && len(examples[c]) < 12 {
			examples[c] = append(examples[c], fmt.Sprintf("in=%s go18=%s goGBK=%s rs18=%s rsGBK=%s",
				r.in, cp(r.g18), cp(r.gbk), cp(r.r18), cp(r.rGBK)))
		}
	}
	// GBK 列专差：0x80-0x9F 前导单字节占比
	c1 := 0
	for _, r := range rows {
		if r.gbk != r.rGBK && len(r.in) == 2 && r.in[0] >= '8' && r.in[0] <= '9' {
			c1++
		}
	}
	fmt.Printf("总行数=%d\n", total)
	keys := make([]string, 0, len(counts))
	for k := range counts {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		fmt.Printf("%-16s %d\n", k, counts[k])
	}
	fmt.Printf("GBK列专差中 0x80-0x9F 前导单字节: %d\n", c1)
	for _, k := range keys {
		if k == "相同" {
			continue
		}
		fmt.Printf("\n[%s] 样例:\n", k)
		for _, e := range examples[k] {
			fmt.Println("  " + e)
		}
	}
	auditValidPairs()
	auditReserved()
	os.Exit(0)
}
