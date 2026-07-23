#include <chrono>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <string>

#include "sherpa-onnx/c-api/c-api.h"

namespace {

int32_t ReportProgress(const float *, int32_t, float progress, void *) {
  std::fprintf(stderr, "PROGRESS %.6f\n", progress);
  return 1;
}

std::string Join(const std::string &left, const char *right) {
  return left + "/" + right;
}

}  // namespace

int main(int argc, char *argv[]) {
  if (argc != 4) {
    std::fprintf(stderr,
                 "Usage: %s MODEL_DIR UTF8_TEXT_FILE OUTPUT_WAV\n",
                 argv[0]);
    return 64;
  }

  const std::string model_dir = argv[1];
  const std::string model_path =
      Join(model_dir, "ru_RU-dmitri-medium.onnx");
  const std::string tokens_path = Join(model_dir, "tokens.txt");
  const std::string data_dir = Join(model_dir, "espeak-ng-data");
  const char *output_path = argv[3];

  std::ifstream input(argv[2], std::ios::binary);
  if (!input) {
    std::fprintf(stderr, "Cannot open text file: %s\n", argv[2]);
    return 65;
  }
  const std::string text((std::istreambuf_iterator<char>(input)),
                         std::istreambuf_iterator<char>());
  if (text.empty()) {
    std::fprintf(stderr, "Text file is empty\n");
    return 66;
  }

  SherpaOnnxOfflineTtsConfig config;
  std::memset(&config, 0, sizeof(config));
  config.model.vits.model = model_path.c_str();
  config.model.vits.tokens = tokens_path.c_str();
  config.model.vits.data_dir = data_dir.c_str();
  config.model.vits.noise_scale = 0.667f;
  config.model.vits.noise_scale_w = 0.8f;
  config.model.vits.length_scale = 1.0f;
  config.model.num_threads = 4;
  config.model.provider = "cpu";
  config.max_num_sentences = 1;
  config.silence_scale = 0.2f;

  const SherpaOnnxOfflineTts *tts = SherpaOnnxCreateOfflineTts(&config);
  if (!tts) {
    std::fprintf(stderr, "Failed to create Piper TTS engine\n");
    return 67;
  }

  SherpaOnnxGenerationConfig generation;
  std::memset(&generation, 0, sizeof(generation));
  generation.speed = 1.0f;
  generation.silence_scale = 0.2f;

  const auto started = std::chrono::steady_clock::now();
  const SherpaOnnxGeneratedAudio *audio =
      SherpaOnnxOfflineTtsGenerateWithConfig(
          tts, text.c_str(), &generation, ReportProgress, nullptr);
  if (!audio) {
    std::fprintf(stderr, "Piper generation failed\n");
    SherpaOnnxDestroyOfflineTts(tts);
    return 68;
  }

  const int32_t written =
      SherpaOnnxWriteWave(audio->samples, audio->n, audio->sample_rate,
                         output_path);
  const double elapsed =
      std::chrono::duration<double>(std::chrono::steady_clock::now() - started)
          .count();
  const double duration =
      static_cast<double>(audio->n) / audio->sample_rate;
  std::fprintf(stderr, "DONE %.3f %.3f %.3f\n", elapsed, duration,
               elapsed / duration);

  SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio);
  SherpaOnnxDestroyOfflineTts(tts);
  return written ? 0 : 69;
}
