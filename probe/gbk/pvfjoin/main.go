package main

import (
	"bufio"
	"fmt"
	"os"
	"strings"
)

type pair struct{ gbk, g18 string }

func load(path string) map[string]pair {
	f, err := os.Open(path)
	if err != nil {
		panic(err)
	}
	defer f.Close()
	r := bufio.NewReaderSize(f, 1<<20)
	m := make(map[string]pair, 1600000)
	for {
		s, err := r.ReadString('\n')
		s = strings.TrimSpace(s)
		if s != "" && !strings.HasPrefix(s, "input_hex") {
			c := strings.Split(s, ",")
			if len(c) >= 3 {
				if _, dup := m[c[0]]; !dup {
					if path[0] == 'p' && strings.HasPrefix(path, "pvf_rs") {
						m[c[0]] = pair{gbk: c[1], g18: c[2]}
					} else {
						m[c[0]] = pair{gbk: c[2], g18: c[1]}
					}
				}
			}
		}
		if err != nil {
			break
		}
	}
	return m
}

func hasFFFD(h string) bool { return strings.Contains(h, "EFBFBD") }
func hasPUA(h string) bool {
	for _, p := range []string{"EE", "EFBC", "F1", "F2", "F3"} {
		if strings.Contains(h, p) {
			return true
		}
	}
	return false
}

func utf16ish(in string) bool {
	if len(in) < 8 {
		return false
	}
	n, total := 0, 0
	for i := 0; i+4 <= len(in); i += 4 {
		total++
		a, b := in[i:i+2] == "00", in[i+2:i+4] == "00"
		if a != b {
			n++
		}
	}
	return total > 0 && float64(n)/float64(total) > 0.6
}

func main() {
	rs := load("pvf_rs.csv")
	gt := load("pvf_go.csv")
	fmt.Printf("rs=%d go=%d\n", len(rs), len(gt))
	cats := map[string]int{}
	tot := map[string]int{}
	var ex [][3]string
	for k, gv := range gt {
		rv, ok := rs[k]
		if !ok {
			continue
		}
		var cat string
		switch {
		case utf16ish(k):
			cat = "UTF16混杂行"
		case hasPUA(gv.gbk) || hasPUA(rv.gbk):
			cat = "含PUA映射"
		case hasFFFD(gv.gbk) || hasFFFD(rv.gbk):
			cat = "含替换符"
		default:
			cat = "纯正常文本"
		}
		tot[cat]++
		if gv.gbk != rv.gbk {
			cats[cat]++
			if cat == "纯正常文本" && len(ex) < 6 {
				ex = append(ex, [3]string{k, gv.gbk, rv.gbk})
			}
		}
	}
	names := []string{"纯正常文本", "UTF16混杂行", "含PUA映射", "含替换符"}
	for _, c := range names {
		fmt.Printf("%-10s: 共 %d 行，go_gbk≠rs_gbk %d 行\n", c, tot[c], cats[c])
	}
	for _, e := range ex {
		fmt.Printf("  in=%s\ngo=%s\nrs=%s\n", e[0], e[1], e[2])
	}
}
