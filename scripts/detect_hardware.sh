#!/usr/bin/env bash
# ==============================================================================
# Cortex Hardware & Environment Capability Detector
# Robust multi-platform hardware profiler with fallbacks for Linux, macOS & WSL.
# ==============================================================================
set -euo pipefail

FORMAT="text"

for arg in "$@"; do
  case "$arg" in
    --json) FORMAT="json" ;;
    --profile-only) FORMAT="profile" ;;
    --help|-h)
      printf "Usage: %s [--json | --profile-only]\n" "$0"
      exit 0
      ;;
  esac
done

detect_os() {
  local os_type="unknown"
  local os_distro="unknown"
  local is_wsl="false"

  local kernel_name
  kernel_name=$(uname -s 2>/dev/null || echo "Unknown")

  if [ "$kernel_name" = "Darwin" ]; then
    os_type="macos"
    if command -v sw_vers >/dev/null 2>&1; then
      os_distro="macOS $(sw_vers -productVersion 2>/dev/null || echo '')"
    else
      os_distro="macOS"
    fi
  elif [ "$kernel_name" = "Linux" ]; then
    os_type="linux"
    if [ -f /proc/version ] && grep -qiE "(microsoft|wsl)" /proc/version; then
      is_wsl="true"
    fi
    if [ -n "${WSL_DISTRO_NAME:-}" ]; then
      is_wsl="true"
    fi

    if [ -f /etc/os-release ]; then
      # shellcheck source=/dev/null
      os_distro=$(. /etc/os-release && echo "${PRETTY_NAME:-$NAME}")
    elif command -v lsb_release >/dev/null 2>&1; then
      os_distro=$(lsb_release -ds 2>/dev/null || echo "Linux")
    else
      os_distro="Linux (generic)"
    fi
  else
    os_type="unsupported"
    os_distro="$kernel_name"
  fi

  printf '%s|%s|%s' "$os_type" "$os_distro" "$is_wsl"
}

detect_cpu() {
  local os_type="$1"
  local arch
  arch=$(uname -m 2>/dev/null || echo "unknown")
  local cores=1
  local model="Unknown CPU"

  if [ "$os_type" = "macos" ]; then
    cores=$(sysctl -n hw.ncpu 2>/dev/null || sysctl -n hw.logicalcpu 2>/dev/null || echo 1)
    model=$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo "$arch")
  else
    if command -v nproc >/dev/null 2>&1; then
      cores=$(nproc 2>/dev/null || echo 1)
    elif [ -f /proc/cpuinfo ]; then
      cores=$(grep -c '^processor' /proc/cpuinfo 2>/dev/null || echo 1)
    elif command -v getconf >/dev/null 2>&1; then
      cores=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)
    fi

    if [ -f /proc/cpuinfo ]; then
      model=$(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^[ \t]*//' || echo "$arch")
    elif command -v lscpu >/dev/null 2>&1; then
      model=$(lscpu 2>/dev/null | grep 'Model name:' | cut -d: -f2- | sed 's/^[ \t]*//' || echo "$arch")
    fi
  fi

  printf '%s|%s|%s' "$arch" "$cores" "$model"
}

detect_ram() {
  local os_type="$1"
  local ram_mb=0

  if [ "$os_type" = "macos" ]; then
    local mem_bytes
    mem_bytes=$(sysctl -n hw.memsize 2>/dev/null || echo 0)
    if [ "$mem_bytes" -gt 0 ] 2>/dev/null; then
      ram_mb=$(( mem_bytes / 1024 / 1024 ))
    fi
  else
    if [ -f /proc/meminfo ]; then
      local mem_kb
      mem_kb=$(grep MemTotal /proc/meminfo 2>/dev/null | awk '{print $2}' || echo 0)
      if [ "$mem_kb" -gt 0 ] 2>/dev/null; then
        ram_mb=$(( mem_kb / 1024 ))
      fi
    elif command -v free >/dev/null 2>&1; then
      ram_mb=$(free -m 2>/dev/null | awk '/^Mem:/{print $2}' || echo 0)
    fi
  fi

  if [ -z "$ram_mb" ] || [ "$ram_mb" -le 0 ]; then
    ram_mb=4096 # safe fallback
  fi

  printf '%s' "$ram_mb"
}

detect_gpu() {
  local os_type="$1"
  local arch="$2"
  local has_gpu="false"
  local gpu_name="None"
  local vram_mb=0
  local has_cuda="false"
  local has_metal="false"

  if [ "$os_type" = "macos" ] && [ "$arch" = "arm64" ]; then
    has_gpu="true"
    has_metal="true"
    gpu_name="Apple Silicon (Unified Memory)"
  fi

  if command -v nvidia-smi >/dev/null 2>&1; then
    local smi_out
    smi_out=$(nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits 2>/dev/null | head -n1 || echo "")
    if [ -n "$smi_out" ]; then
      has_gpu="true"
      has_cuda="true"
      gpu_name=$(echo "$smi_out" | cut -d, -f1 | sed 's/^[ \t]*//')
      vram_mb=$(echo "$smi_out" | cut -d, -f2 | sed 's/^[ \t]*//' || echo 0)
    fi
  fi

  if [ "$has_gpu" = "false" ] && command -v lspci >/dev/null 2>&1; then
    local pci_gpu
    pci_gpu=$(lspci 2>/dev/null | grep -iE 'vga|3d|display' | head -n1 || echo "")
    if [ -n "$pci_gpu" ]; then
      has_gpu="true"
      gpu_name=$(echo "$pci_gpu" | cut -d: -f3- | sed 's/^[ \t]*//')
      if echo "$gpu_name" | grep -qi nvidia; then
        has_cuda="true"
      fi
    fi
  fi

  printf '%s|%s|%s|%s|%s' "$has_gpu" "$gpu_name" "$vram_mb" "$has_cuda" "$has_metal"
}

evaluate_profile() {
  local ram_mb="$1"
  local cores="$2"
  local has_cuda="$3"
  local has_metal="$4"
  local vram_mb="$5"

  # Light: RAM < 7500 MB or cores < 4 (and no dedicated acceleration)
  # Complete: (RAM >= 15000 MB AND cores >= 6) OR (CUDA with >= 5.5GB VRAM) OR (Apple Silicon with >= 15GB RAM)
  # Medium: All balanced middle cases

  if [ "$ram_mb" -lt 7500 ] && [ "$has_cuda" = "false" ] && [ "$has_metal" = "false" ]; then
    printf 'light'
  elif [ "$cores" -lt 4 ] && [ "$has_cuda" = "false" ] && [ "$has_metal" = "false" ]; then
    printf 'light'
  elif [ "$has_cuda" = "true" ] && [ "$vram_mb" -ge 5500 ]; then
    printf 'complete'
  elif [ "$has_metal" = "true" ] && [ "$ram_mb" -ge 15000 ]; then
    printf 'complete'
  elif [ "$ram_mb" -ge 15000 ] && [ "$cores" -ge 6 ]; then
    printf 'complete'
  else
    printf 'medium'
  fi
}

main() {
  local os_info
  os_info=$(detect_os)
  local os_type os_distro is_wsl
  os_type=$(echo "$os_info" | cut -d'|' -f1)
  os_distro=$(echo "$os_info" | cut -d'|' -f2)
  is_wsl=$(echo "$os_info" | cut -d'|' -f3)

  local cpu_info
  cpu_info=$(detect_cpu "$os_type")
  local cpu_arch cpu_cores cpu_model
  cpu_arch=$(echo "$cpu_info" | cut -d'|' -f1)
  cpu_cores=$(echo "$cpu_info" | cut -d'|' -f2)
  cpu_model=$(echo "$cpu_info" | cut -d'|' -f3)

  local ram_mb
  ram_mb=$(detect_ram "$os_type")
  local ram_gb
  ram_gb=$(awk "BEGIN {printf \"%.1f\", $ram_mb/1024}")

  local gpu_info
  gpu_info=$(detect_gpu "$os_type" "$cpu_arch")
  local has_gpu gpu_name vram_mb has_cuda has_metal
  has_gpu=$(echo "$gpu_info" | cut -d'|' -f1)
  gpu_name=$(echo "$gpu_info" | cut -d'|' -f2)
  vram_mb=$(echo "$gpu_info" | cut -d'|' -f3)
  has_cuda=$(echo "$gpu_info" | cut -d'|' -f4)
  has_metal=$(echo "$gpu_info" | cut -d'|' -f5)

  local recommended_profile
  recommended_profile=$(evaluate_profile "$ram_mb" "$cpu_cores" "$has_cuda" "$has_metal" "$vram_mb")

  if [ "$FORMAT" = "profile" ]; then
    printf '%s\n' "$recommended_profile"
  elif [ "$FORMAT" = "json" ]; then
    cat <<EOF
{
  "os": {
    "type": "$os_type",
    "distro": "$os_distro",
    "is_wsl": $is_wsl
  },
  "cpu": {
    "arch": "$cpu_arch",
    "cores": $cpu_cores,
    "model": "$cpu_model"
  },
  "ram": {
    "total_mb": $ram_mb,
    "total_gb": $ram_gb
  },
  "gpu": {
    "available": $has_gpu,
    "name": "$gpu_name",
    "vram_mb": $vram_mb,
    "cuda": $has_cuda,
    "metal": $has_metal
  },
  "recommended_profile": "$recommended_profile"
}
EOF
  else
    printf "========================================================\n"
    printf " Cortex System & Hardware Capabilities Profile\n"
    printf "========================================================\n"
    printf " OS:            %s (%s)\n" "$os_distro" "$os_type"
    if [ "$is_wsl" = "true" ]; then
      printf " Environment:   Windows Subsystem for Linux (WSL2)\n"
    fi
    printf " CPU:           %s (%s cores, %s)\n" "$cpu_model" "$cpu_cores" "$cpu_arch"
    printf " Memory:        %s GB (%s MB)\n" "$ram_gb" "$ram_mb"
    printf " GPU:           %s\n" "$gpu_name"
    if [ "$has_cuda" = "true" ]; then
      printf " Acceleration:  NVIDIA CUDA (VRAM: %s MB)\n" "$vram_mb"
    elif [ "$has_metal" = "true" ]; then
      printf " Acceleration:  Apple Metal (Unified Memory)\n"
    else
      printf " Acceleration:  None (CPU Only)\n"
    fi
    printf '%s\n' "--------------------------------------------------------"
    printf " Recommended Profile: %s\n" "$recommended_profile"
    printf '%s\n' "========================================================"
  fi
}

main "$@"
