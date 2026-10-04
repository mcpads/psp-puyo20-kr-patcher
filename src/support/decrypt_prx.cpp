// SPDX-License-Identifier: MIT
// Thin local executable around the separately supplied, pinned PPSSPP decryptor.
#include <cstdio>
#include <fstream>
#include <iterator>
#include <vector>
#include "Common/Log.h"
#include "Common/Swap.h"
#include "Core/ELF/PrxDecrypter.h"

static bool logging_enabled = false;
bool *g_bLogEnabledSetting = &logging_enabled;
LogChannel g_log[(size_t)Log::NUMBER_OF_LOGS];
void GenericLog(Log, LogLevel, const char *, int, const char *, ...) {}

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    std::ifstream input(argv[1], std::ios::binary);
    if (!input) return 3;
    std::vector<u8> encrypted((std::istreambuf_iterator<char>(input)), {});
    // The Rust caller verifies the exact source before this native parser runs.
    if (encrypted.size() != 2717200) return 4;
    std::vector<u8> plain(encrypted.size(), 0);
    int size = pspDecryptPRX(encrypted.data(), plain.data(), (u32)encrypted.size());
    if (size <= 0 || (size_t)size > plain.size()) {
        std::fprintf(stderr, "PRX decrypt failed: %d\n", size);
        return 5;
    }
    std::ofstream output(argv[2], std::ios::binary);
    output.write(reinterpret_cast<const char *>(plain.data()), size);
    return output ? 0 : 6;
}
