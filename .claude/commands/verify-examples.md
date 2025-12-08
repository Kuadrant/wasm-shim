description: Verify that all examples work correctly

Verify that all examples in the `examples/` directory work as documented:

## Steps to Execute

1. **Check Prerequisites**
   - Verify Docker is running: `docker info`
   - Build the WASM module with `make build` (unless `--skip-build` is specified)
   - This ensures the examples are tested with the latest code

2. **For Each Example Directory** (examples/ratelimit, examples/ratelimit_check_report)
   
   a. **Extract Test Information from README.md**
      - Parse curl commands (lines starting with `curl --resolve`)
      - Identify expected responses (e.g., "should return `200 OK`")
      - Extract expected log patterns if mentioned (e.g., "entries: [Entry")
   
   b. **Start the Example**
      - Run: `make run` in the example directory
      - This command waits for all services to be ready before returning (via the start_services container)
   
   c. **Execute Test Requests**
      - Run each curl command found in the README multiple times (at least 2-3 times)
      - Validate response status codes (expect 200 unless README says otherwise)
      - For streaming responses (sse-streaming example), verify `Content-Type: text/event-stream`
      - Verify hits_addend values are correct for each request in the logs
   
   d. **Optional: Verify Logs** (if time permits)
      - Check limitador logs for expected patterns mentioned in README
      - Look for descriptor entries or hits_addend values
   
   e. **Cleanup**
      - Run: `make clean` in the example directory
      - Verify all containers are stopped

3. **Report Results**
   - Summary table showing which examples passed/failed
   - For failures, show:
     - What failed (build, startup, curl command, expected response)
     - Actual vs expected results
     - Relevant error messages

## Options to Support

- `--example <name>`: Test only specific example (e.g., `--example ratelimit`)
- `--skip-build`: Skip building WASM module (assume it exists)
- `--no-cleanup`: Leave containers running for debugging
- `--verbose`: Show full curl output and logs

## Important Notes

- Run this from the repository root directory
- Each example should be tested in isolation (clean up between examples)
- Don't fail fast - run all examples and report all results
- The docker-compose setup uses a `start_services` helper that waits for readiness
- Port 18000 is used for Envoy in all examples
- Be patient - Docker Compose startup can take 10-30 seconds

## Expected Behavior

For `examples/ratelimit`:
- Multiple curls to `ratelimit.example.com:18000/path`
- Should return 200 OK for each request
- Limitador logs should show descriptor entry `entries: [Entry { key: "a", value: "1" }]` for each request
- Each request should have `hits_addend: 1` in the logs

For `examples/ratelimit_check_report`:
- Two different test scenarios (JSON and SSE streaming), each tested multiple times
- Both curl to port 18000 with different hostnames
- Should return 200 OK for each request
- For each request, limitador logs should show:
  - CheckRateLimit request with `hits_addend: 1` (initial check before processing)
  - Report request with `hits_addend` matching `usage.total_tokens` from the response body
- Verify token counts are accurately reported

## Error Handling

If verification fails:
- Capture docker compose logs for debugging
- Check if ports 18000, 18001, 18080, 18081, 3000 are already in use
- Suggest running `make clean` if containers are in a bad state
- Recommend checking Docker has enough resources
