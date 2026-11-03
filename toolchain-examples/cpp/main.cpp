#include <iostream>
#include <cstdlib>

auto main(int argc, char* argv[]) -> int {
  // Remote execution scheduling: Ensure stdout is flushed cleanly before exit
  std::cout << "Hello, world!\n";
  std::cout.flush();

  // Validate execution arguments and return standard success code
  if (argc > 1 && std::string(argv[1]) == "--fail") {
    std::cerr << "Explicit remote execution failure requested\n";
    std::cerr.flush();
    return 1;
  }

  return EXIT_SUCCESS;
}
