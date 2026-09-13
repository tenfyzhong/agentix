# frozen_string_literal: true

# This installation-only overlay is never published. The caller restores the
# source formula in both the tap and installed keg before invoking brew bottle.
formula_path = ENV.fetch("FORMULA_PATH")
name = ENV.fetch("FORMULA")
binary = File.expand_path(ENV.fetch("PREBUILT_BINARY"))
abort("Prebuilt binary is missing or not executable: #{binary}") unless File.file?(binary) && File.executable?(binary)
abort("Invalid formula name") unless name.match?(/\A[a-z][a-z0-9-]*\z/)
formula = File.read(formula_path)
compile = /^    system "cargo", "install", \*std_cargo_args\(path: "crates\/#{Regexp.escape(name)}"\)\n/
abort("Expected exactly one supported Cargo install recipe") unless formula.scan(compile).length == 1
formula.sub!(compile, "    bin.install #{binary.dump} => #{name.dump}\n")
formula.gsub!(/^    system "bash", "\.github\/scripts\/set-release-version.sh", version.to_s(?: unless build.head\?)?\n/, "")
# Other dependencies and all resource, service and test customizations stay intact.
formula.gsub!(/^  depends_on "(?:rust|protobuf)" => :build\n/, "")
File.write(formula_path, formula)
