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

# Число шагов RUN в логе BuildKit, у которых нет отметки CACHED.
uncached_run_steps() {
  local log=$1 n=0 id
  for id in $(grep -oE '^#[0-9]+ \[[^]]+\] RUN' "$log" | grep -oE '^#[0-9]+' | sort -u); do
    grep -qE "^${id} CACHED" "$log" || n=$((n + 1))
  done
  echo "$n"
}

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

  # После правки кода шаг установки зависимостей обязан взяться из кеша.
  step=$(grep -F "${DEPS_STEP[$svc]}" "/tmp/$svc-edit.log" | grep -oE '^#[0-9]+' | head -n1 || true)
  if [[ -n "$step" ]] && grep -qE "^${step} CACHED" "/tmp/$svc-edit.log"; then
    cached="да"
  else
    cached="**НЕТ**"
  fi
  # Без изменений из кеша обязан взяться КАЖДЫЙ шаг RUN, а не хотя бы один.
  uncached=$(uncached_run_steps "/tmp/$svc-warm.log")
  size=$(docker image ls "$tag" --format '{{.Size}}')

  echo "| $svc | $(seconds $((t1 - t0))) | $(seconds $((t2 - t1))) | $(seconds $((t3 - t2))) | $cached | $size |"
  if [[ "$cached" != "да" ]]; then
    echo "::error::$svc: слой зависимостей пересобран после правки кода" >&2
    cat "/tmp/$svc-edit.log" >&2
    exit 1
  fi
  if [[ "$uncached" -ne 0 ]]; then
    echo "::error::$svc: при повторной сборке без изменений $uncached шагов RUN выполнены заново" >&2
    cat "/tmp/$svc-warm.log" >&2
    exit 1
  fi
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
