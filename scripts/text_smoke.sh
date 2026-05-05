#!/usr/bin/env bash
set -euo pipefail

printf '\033[1mTermite text smoke test\033[0m\n\n'

printf '\033[36mASCII and punctuation\033[0m\n'
printf "plain ASCII: the quick brown fox jumps over 1234567890\n"
printf "straight quotes: we'll \"quote\" 'things'\n"
printf $'smart quotes fallback: we\u2019ll \u201chello\u201d \u2018shell\u2019\n'
printf $'smart dashes fallback: one\u2013two three\u2014four wait\u2026 done\n\n'

printf '\033[36mANSI styles\033[0m\n'
printf '\033[31mred\033[0m \033[32mgreen\033[0m \033[33myellow\033[0m \033[34mblue\033[0m \033[35mmagenta\033[0m \033[36mcyan\033[0m\n'
printf '\033[1mbold\033[0m \033[3mitalic\033[0m \033[4munderline\033[0m \033[1;4mbold underline\033[0m\n'
printf '\033[38;2;255;120;80mtruecolor foreground\033[0m '
printf '\033[48;2;45;65;95mtruecolor background\033[0m\n\n'

printf '\033[36mBox drawing\033[0m\n'
printf $'\u250c\u2500\u2500\u2500\u252c\u2500\u2500\u2500\u2510  \u256d\u2500\u2500\u2500\u256e\n'
printf $'\u2502 A \u2502 B \u2502  \u2502   \u2502\n'
printf $'\u251c\u2500\u2500\u2500\u253c\u2500\u2500\u2500\u2524  \u2570\u2500\u2500\u2500\u256f\n'
printf $'\u2502 C \u2502 D \u2502\n'
printf $'\u2514\u2500\u2500\u2500\u2534\u2500\u2500\u2500\u2518\n\n'

printf '\033[36mBlocks and shades\033[0m\n'
printf $'full and half: \u2588\u2588\u2588 \u2580\u2580\u2580 \u2584\u2584\u2584 \u258c\u258c \u2590\u2590\n'
printf $'quadrants:     \u2598 \u259d \u2596 \u2597 \u259a \u259e \u2599 \u259b \u259c \u259f\n'
printf $'shades:        \u2591\u2591\u2591 \u2592\u2592\u2592 \u2593\u2593\u2593\n\n'

printf '\033[36mUnicode and symbol fallback\033[0m\n'
printf $'latin: cafe\u0301 naive facade jalapen\u0303o\n'
printf $'math:  \u03c0 \u03bb \u2211 \u221a \u2248 \u2260 \u2264 \u2265 \u2190 \u2191 \u2192 \u2193\n'
printf $'icons: \ue0b0 \ue0b1 \uf120 \uf121 \uf013 \uf07b \uf15b \uf1c0\n\n'

printf '\033[36mCursor/key checks\033[0m\n'
printf 'Try Shift-Tab, F1-F12, Ctrl-Arrow, Shift-Arrow, and Alt-Arrow in your shell/editor.\n'
printf 'If smart punctuation still shows as ?, the fallback path is not being used.\n'
