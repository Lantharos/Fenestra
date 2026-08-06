#ifndef SABINE_CEF_HOST_JSON_UTIL_H_
#define SABINE_CEF_HOST_JSON_UTIL_H_

#include <set>
#include <string>
#include <vector>

std::string JsonEscape(const std::string& value);
std::string JsString(const std::string& value);
std::string JsArray(const std::set<std::string>& values);

bool JsonHasKey(const std::string& payload, const std::string& name);
std::string JsonStringValue(const std::string& payload, const std::string& name);
std::string JsonObjectValue(const std::string& payload, const std::string& name);
std::vector<std::string> JsonStringArrayValue(const std::string& payload,
                                               const std::string& name);
int JsonIntValue(const std::string& payload, const std::string& name, int fallback);
double JsonDoubleValue(const std::string& payload,
                       const std::string& name,
                       double fallback);
bool JsonBoolValue(const std::string& payload,
                   const std::string& name,
                   bool fallback);

std::string JsonMessage(const std::string& message);

#endif
