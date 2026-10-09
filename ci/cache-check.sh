#!/usr/bin/env bash
# Проверка оптимизации слоёв (повышенное задание 5).
#
# Для каждого образа:
#   1. холодная сборка без кеша;
#   2. повторная сборка без изменений — всё должно взяться из кеша;
#   3. правка только исходного кода — слой зависимостей ОБЯЗАН взяться из кеша,
#      иначе скрипт завершается с ошибкой.
# Результат — таблица Markdown (в CI она попадает в summary).
set -euo pipefail
cd "$(dirname "$0")/.."

declare -A SOURCE=([ingest]=main.go [processor]=src/main.rs [reports]=app/main.py)
declare -A COMMENT=([ingest]="//" [processor]="//" [reports]="#")
# Уникальный фрагмент команды RUN, которая ставит зависимости.
declare -A DEPS_STEP=(
  [ingest]="go mod download"
  [processor]="echo 'fn main() {}'"
  [reports]="pip install"
)

build() { # service tag [extra args] -> печатает лог сборки
  local svc=$1 tag=$2; shift 2
  DOCKER_BUILDKIT=1 docker build --progress=plain -t "$tag" "$@" "$svc" 2>&1
}

seconds() { awk "BEGIN {printf \"%.1f\", $1/1000000000}"; }

echo "| Образ | Холодная сборка, с | Без изменений, с | После правки кода, с | Слой зависимостей из кеша | Размер |"
echo "|---|---:|---:|---:|:---:|---:|"

for svc in ingest processor reports; do
  tag="cache-check/$svc"
  src="$svc/${SOURCE[$svc]}"

  t0=$(date +%s%N); build "$svc" "$tag" --no-cache > "/tmp/$svc-cold.log"; t1=$(date +%s%N)
  build "$svc" "$tag" > "/tmp/$svc-warm.log"; t2=$(date +%s%N)

  cp "$src" "/tmp/$svc.bak"
  echo "${COMMENT[$svc]} cache-check $(date +%s%N)" >> "$src"
  build "$svc" "$tag" > "/tmp/$svc-edit.log"; t3=$(date +%s%N)
  cp "/tmp/$svc.bak" "$src"

  # Номер шага BuildKit с установкой зависимостей и проверка, что он CACHED.
  step=$(grep -F "${DEPS_STEP[$svc]}" "/tmp/$svc-edit.log" | grep -oE '^#[0-9]+' | head -n1 || true)
  if [[ -n "$step" ]] && grep -qE "^${step} CACHED" "/tmp/$svc-edit.log"; then
    cached="да"
  else
    cached="**НЕТ**"
  fi
  # Без изменений не должен выполниться ни один шаг RUN.
  if grep -qE '^#[0-9]+ \[.*\] RUN' "/tmp/$svc-warm.log" && ! grep -qE '^#[0-9]+ CACHED' "/tmp/$svc-warm.log"; then
    cached="**НЕТ (повторная сборка не из кеша)**"
  fi
  size=$(docker image ls "$tag" --format '{{.Size}}')

  echo "| $svc | $(seconds $((t1 - t0))) | $(seconds $((t2 - t1))) | $(seconds $((t3 - t2))) | $cached | $size |"
  [[ "$cached" == "да" ]] || { echo "::error::$svc: слой зависимостей пересобран после правки кода" >&2; cat "/tmp/$svc-edit.log" >&2; exit 1; }
done

echo
echo "#### Слои финальных образов"
for svc in ingest processor reports; do
  echo
  echo "<details><summary>$svc</summary>"
  echo
  echo '```'
  docker history --format '{{.Size}}\t{{.CreatedBy}}' "cache-check/$svc" | cut -c1-140
  echo '```'
  echo "</details>"
done
