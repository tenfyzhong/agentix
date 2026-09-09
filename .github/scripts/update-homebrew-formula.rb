# frozen_string_literal: true

formula_path = ENV.fetch("FORMULA_PATH")
formula = File.read(formula_path)
source_url = ENV.fetch("SOURCE_URL")
source_sha256 = ENV.fetch("SOURCE_SHA256")
previous_source_url = formula[/^  url "(.*)"$/, 1]
if previous_source_url
  formula.sub!(/^  url ".*"$/, %(  url "#{source_url}"))
  formula.sub!(/^  sha256 ".*"$/, %(  sha256 "#{source_sha256}")) or abort("Formula SHA-256 not found")
else
  formula.sub!(/^  homepage ".*"$/) do |homepage|
    %(#{homepage}\n  url "#{source_url}"\n  sha256 "#{source_sha256}")
  end or abort("Formula homepage not found")
end
formula.sub!(/^  revision \d+\n/, "") if previous_source_url != source_url
formula.sub!(/^  bottle do\n.*?^  end\n\n?/m, "")
File.write(formula_path, formula)
