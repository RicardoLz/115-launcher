// GB18030 / WHATWG gbk 差异探针（Go 侧 = 参考实现）。
package main

import (
	"bufio"
	"encoding/csv"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"strings"

	"golang.org/x/text/encoding"
	"golang.org/x/text/encoding/simplifiedchinese"
	"golang.org/x/text/transform"
)

func decodeNew(enc encoding.Encoding, in []byte) (string, error) {
	out, _, err := transform.Bytes(enc.NewDecoder(), in)
	if err != nil {
		return "", err
	}
	return string(out), nil
}

// cmd gen：枚举合成输入 -> inputs.csv（每行一个小写 hex 序列）
func cmdGen() error {
	f, err := os.Create("inputs.csv")
	if err != nil {
		return err
	}
	defer f.Close()
	w := bufio.NewWriter(f)
	defer w.Flush()
	emit := func(b ...byte) {
		fmt.Fprintf(w, "%x\n", b)
	}
	emit()
	for hi := 0x80; hi <= 0xFF; hi++ {
		emit(byte(hi))
	}
	for hi := 0x80; hi <= 0xFF; hi++ {
		for lo := 0x00; lo <= 0xFF; lo++ {
			if hi < 0x81 && (lo < 0x40 || lo == 0x7F) {
				continue
			}
			emit(byte(hi), byte(lo))
		}
	}
	for a := 0x81; a <= 0xFE; a++ {
		for b := 0x30; b <= 0x39; b++ {
			for c := 0x81; c <= 0xFE; c++ {
				for d := 0x30; d <= 0x39; d++ {
					emit(byte(a), byte(b), byte(c), byte(d))
				}
			}
		}
	}
	for _, s := range []string{
		"cbd6b9fa", "a1a1", "80", "8080", "d6b9fa80", "c8b9fcd6b9", "b9fcd7d4",
		"c8cbd3ef80a3", "d3a2ceaa", "a3a1", "d6b0", "81308130", "8130fe39", "ffd4d320",
	} {
		fmt.Fprintf(w, "%s\n", s)
	}
	return nil
}

// cmd go：读 inputs.csv -> go_out.csv（input_hex, go_gb18030_hex, go_gbk_hex）
func cmdGo() error {
	rf, err := os.Open("inputs.csv")
	if err != nil {
		return err
	}
	defer rf.Close()
	wf, err := os.Create("go_out.csv")
	if err != nil {
		return err
	}
	defer wf.Close()
	br := bufio.NewReader(rf)
	bw := bufio.NewWriterSize(wf, 1<<20)
	defer bw.Flush()
	w := csv.NewWriter(bw)
	if err := w.Write([]string{"input_hex", "go_gb18030_hex", "go_gbk_hex"}); err != nil {
		return err
	}
	n := 0
	for {
		line, err := br.ReadString('\n')
		if s := strings.TrimSpace(line); s != "" {
			b, decErr := hex.DecodeString(s)
			if decErr != nil {
				return fmt.Errorf("inputs.csv 第 %d 行非法: %v", n+2, decErr)
			}
			g18, e1 := decodeNew(simplifiedchinese.GB18030, b)
			gbk, e2 := decodeNew(simplifiedchinese.GBK, b)
			rec := []string{s, cell(b, g18, e1), cell(b, gbk, e2)}
			if err := w.Write(rec); err != nil {
				return err
			}
			n++
		}
		if err == io.EOF {
			break
		} else if err != nil {
			return err
		}
	}
	w.Flush()
	fmt.Printf("go_out.csv：%d 行\n", n)
	return w.Error()
}

func cell(in []byte, s string, err error) string {
	if err != nil {
		return "ERR:" + err.Error()
	}
	return strings.ToUpper(hex.EncodeToString([]byte(s)))
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "用法: go run . gen|go")
		os.Exit(2)
	}
	var err error
	switch os.Args[1] {
	case "gen":
		err = cmdGen()
	case "go":
		err = cmdGo()
	case "pvf":
		err = cmdPvf()
	default:
		err = fmt.Errorf("未知子命令 %s", os.Args[1])
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

// cmd pvf：读 pvf_lines.csv -> pvf_go.csv（input_hex, go_gb18030_hex, go_gbk_hex）
func cmdPvf() error {
	rf, err := os.Open("pvf_lines.csv")
	if err != nil {
		return err
	}
	defer rf.Close()
	wf, err := os.Create("pvf_go.csv")
	if err != nil {
		return err
	}
	defer wf.Close()
	br := bufio.NewReaderSize(rf, 1<<20)
	bw := bufio.NewWriterSize(wf, 1<<20)
	defer bw.Flush()
	w := csv.NewWriter(bw)
	n := 0
	for {
		line, err := br.ReadString('\n')
		if s := strings.TrimSpace(line); s != "" {
			b, e := hex.DecodeString(s)
			if e != nil {
				return e
			}
			g18, _ := decodeNew(simplifiedchinese.GB18030, b)
			gbk, _ := decodeNew(simplifiedchinese.GBK, b)
			if e := w.Write([]string{s, strings.ToUpper(hex.EncodeToString([]byte(g18))), strings.ToUpper(hex.EncodeToString([]byte(gbk)))}); e != nil {
				return e
			}
			n++
		}
		if err == io.EOF {
			break
		} else if err != nil {
			return err
		}
	}
	w.Flush()
	fmt.Printf("pvf_go.csv：%d 行\n", n)
	return w.Error()
}
