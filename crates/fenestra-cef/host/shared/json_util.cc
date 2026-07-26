#include "json_util.h"

#include <cctype>
#include <cstdio>
#include <cstdlib>

namespace {

size_t FindValueStart(const std::string& payload, const std::string& name) {
  const std::string needle = "\"" + name + "\"";
  size_t cursor = payload.find(needle);
  if (cursor == std::string::npos) {
    return std::string::npos;
  }
  cursor = payload.find(':', cursor + needle.size());
  if (cursor == std::string::npos) {
    return std::string::npos;
  }
  ++cursor;
  while (cursor < payload.size() &&
         std::isspace(static_cast<unsigned char>(payload[cursor]))) {
    ++cursor;
  }
  return cursor < payload.size() ? cursor : std::string::npos;
}

std::string NumberLiteral(const std::string& payload, size_t cursor) {
  size_t end = cursor;
  if (end < payload.size() && (payload[end] == '-' || payload[end] == '+')) {
    ++end;
  }
  while (end < payload.size() &&
         (std::isdigit(static_cast<unsigned char>(payload[end])) ||
          payload[end] == '.' || payload[end] == 'e' || payload[end] == 'E' ||
          ((payload[end] == '-' || payload[end] == '+') && end > cursor &&
           (payload[end - 1] == 'e' || payload[end - 1] == 'E')))) {
    ++end;
  }
  return payload.substr(cursor, end - cursor);
}

}  // namespace

std::string JsonEscape(const std::string& value) {
  std::string output;
  output.reserve(value.size() + 2);
  for (char c : value) {
    switch (c) {
      case '\\':
        output += "\\\\";
        break;
      case '"':
        output += "\\\"";
        break;
      case '\n':
        output += "\\n";
        break;
      case '\r':
        output += "\\r";
        break;
      case '\t':
        output += "\\t";
        break;
      default:
        if (static_cast<unsigned char>(c) < 0x20) {
          char buffer[8];
          std::snprintf(buffer, sizeof(buffer), "\\u%04x",
                        static_cast<unsigned char>(c));
          output += buffer;
        } else {
          output += c;
        }
        break;
    }
  }
  return output;
}

std::string JsString(const std::string& value) {
  return "\"" + JsonEscape(value) + "\"";
}

std::string JsArray(const std::set<std::string>& values) {
  std::string output = "[";
  bool first = true;
  for (const auto& value : values) {
    if (!first) {
      output += ",";
    }
    output += JsString(value);
    first = false;
  }
  output += "]";
  return output;
}

bool JsonHasKey(const std::string& payload, const std::string& name) {
  return FindValueStart(payload, name) != std::string::npos;
}

std::string JsonStringValue(const std::string& payload,
                            const std::string& name) {
  size_t cursor = FindValueStart(payload, name);
  if (cursor == std::string::npos || payload[cursor] != '"') {
    return "";
  }
  std::string output;
  for (++cursor; cursor < payload.size(); ++cursor) {
    const char c = payload[cursor];
    if (c == '"') {
      break;
    }
    if (c != '\\' || cursor + 1 >= payload.size()) {
      output += c;
      continue;
    }
    const char escaped = payload[++cursor];
    switch (escaped) {
      case 'n':
        output += '\n';
        break;
      case 'r':
        output += '\r';
        break;
      case 't':
        output += '\t';
        break;
      default:
        output += escaped;
        break;
    }
  }
  return output;
}

std::string JsonObjectValue(const std::string& payload,
                            const std::string& name) {
  const size_t start = FindValueStart(payload, name);
  if (start == std::string::npos || payload[start] != '{') {
    return "";
  }
  int depth = 0;
  bool in_string = false;
  for (size_t cursor = start; cursor < payload.size(); ++cursor) {
    const char c = payload[cursor];
    if (in_string) {
      if (c == '\\') {
        ++cursor;
      } else if (c == '"') {
        in_string = false;
      }
      continue;
    }
    if (c == '"') {
      in_string = true;
    } else if (c == '{') {
      ++depth;
    } else if (c == '}') {
      if (--depth == 0) {
        return payload.substr(start, cursor - start + 1);
      }
    }
  }
  return "";
}

int JsonIntValue(const std::string& payload,
                 const std::string& name,
                 int fallback) {
  const size_t cursor = FindValueStart(payload, name);
  if (cursor == std::string::npos) {
    return fallback;
  }
  const std::string literal = NumberLiteral(payload, cursor);
  if (literal.empty()) {
    return fallback;
  }
  return static_cast<int>(std::strtod(literal.c_str(), nullptr));
}

double JsonDoubleValue(const std::string& payload,
                       const std::string& name,
                       double fallback) {
  const size_t cursor = FindValueStart(payload, name);
  if (cursor == std::string::npos) {
    return fallback;
  }
  const std::string literal = NumberLiteral(payload, cursor);
  if (literal.empty()) {
    return fallback;
  }
  return std::strtod(literal.c_str(), nullptr);
}

bool JsonBoolValue(const std::string& payload,
                   const std::string& name,
                   bool fallback) {
  const size_t cursor = FindValueStart(payload, name);
  if (cursor == std::string::npos) {
    return fallback;
  }
  if (payload.compare(cursor, 4, "true") == 0) {
    return true;
  }
  if (payload.compare(cursor, 5, "false") == 0) {
    return false;
  }
  return fallback;
}

std::string JsonMessage(const std::string& message) {
  return "{\"message\":\"" + JsonEscape(message) + "\"}";
}
