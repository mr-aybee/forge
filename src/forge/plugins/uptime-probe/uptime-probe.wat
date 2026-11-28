;; Sample Orbyn Forge plugin: "uptime probe".
;;
;; A WebAssembly component (text format) targeting the `forge:plugin/plugin`
;; world. It logs a line and calls the typed host function
;; `server-online("web-01")`, returning 1 if the server is online, else 0.
;;
;; Written in WAT so the repository needs no wasm toolchain; plugins in Rust
;; use wit-bindgen against src/protocol/forge-sdk/wit/forge-plugin.wit instead.

(component
  (import "forge:plugin/host@0.1.0" (instance $host
    (export "log" (func (param "message" string)))
    (export "server-online" (func (param "hostname" string) (result bool)))
  ))

  ;; Linear memory lives in its own module so it can be shared with the
  ;; canonical ABI lowering of the host functions.
  (core module $Memory (memory (export "memory") 1))
  (core instance $memory (instantiate $Memory))
  (alias core export $memory "memory" (core memory $mem))

  (alias export $host "log" (func $log_f))
  (alias export $host "server-online" (func $server_online_f))
  (core func $log (canon lower (func $log_f) (memory $mem)))
  (core func $server_online (canon lower (func $server_online_f) (memory $mem)))

  (core module $Main
    (import "env" "memory" (memory 1))
    (import "host" "log" (func $log (param i32 i32)))
    (import "host" "server-online" (func $server_online (param i32 i32) (result i32)))
    (data (i32.const 0) "web-01")
    (data (i32.const 16) "uptime-probe: checking web-01")
    (func (export "run") (param $input i32) (result i32)
      (call $log (i32.const 16) (i32.const 29))
      (call $server_online (i32.const 0) (i32.const 6))))

  (core instance $main (instantiate $Main
    (with "env" (instance $memory))
    (with "host" (instance
      (export "log" (func $log))
      (export "server-online" (func $server_online))))))

  (func (export "run") (param "input" u32) (result u32)
    (canon lift (core func $main "run")))
)
