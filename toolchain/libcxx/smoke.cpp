// The C++ runtimes against mlibc (toolchain/libcxx), each piece a C++
// port leans on: exceptions thrown through frames and caught by base
// class (libunwind's unwinder finding them through dl_iterate_phdr,
// libc++abi's personality), one carried across threads in an
// exception_ptr; RTTI (dynamic_cast, typeid); a global's constructor run
// before main; std::thread, mutex, condition_variable and atomic wait;
// thread_local objects constructed per thread and destroyed at its exit;
// the containers, string formatting (iostream, std::format, to_chars);
// <filesystem>, <chrono>, <random>. Prints `[ OK ] cxx`, or the first
// check that failed (`[ FAIL ] cxx: ...`).

#include <atomic>
#include <charconv>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <exception>
#include <filesystem>
#include <format>
#include <map>
#include <memory>
#include <mutex>
#include <random>
#include <sstream>
#include <stdexcept>
#include <string>
#include <thread>
#include <typeinfo>
#include <unordered_map>
#include <vector>

namespace {

const char* failed = nullptr;

void check(bool ok, const char* what) {
    if (!ok && failed == nullptr) {
        failed = what;
    }
}

// A global's constructor runs before main.
struct Early {
    int value = 0;
    Early() { value = 42; }
};
Early early;

struct Base {
    virtual ~Base() = default;
};
struct Derived : Base {
    int x = 7;
};

struct MyError : std::runtime_error {
    int code;
    explicit MyError(int c) : std::runtime_error("my error"), code(c) {}
};

[[gnu::noinline]] void thrower(int depth) {
    if (depth == 0) {
        throw MyError(5);
    }
    std::string keep(64, 'x');  // a frame with a destructor to unwind
    thrower(depth - 1);
    keep.push_back('y');
}

void exceptions() {
    int caught = 0;
    try {
        thrower(8);
    } catch (const std::runtime_error& e) {
        auto* mine = dynamic_cast<const MyError*>(&e);
        caught = mine != nullptr ? mine->code : -1;
    }
    check(caught == 5, "a throw through 8 frames caught by base class");

    try {
        throw 17;
    } catch (int v) {
        caught = v;
    }
    check(caught == 17, "an int thrown and caught");

    try {
        std::vector<int> v;
        (void)v.at(3);
        caught = 0;
    } catch (const std::out_of_range&) {
        caught = 1;
    }
    check(caught == 1, "std::out_of_range from vector::at");

    // An exception thrown on a thread and rethrown on this one.
    std::exception_ptr ep;
    std::thread t([&] {
        try {
            throw std::logic_error("from a thread");
        } catch (...) {
            ep = std::current_exception();
        }
    });
    t.join();
    std::string what;
    try {
        std::rethrow_exception(ep);
    } catch (const std::logic_error& e) {
        what = e.what();
    }
    check(what == "from a thread", "an exception_ptr rethrown on another thread");
}

void rtti() {
    std::unique_ptr<Base> b = std::make_unique<Derived>();
    auto* d = dynamic_cast<Derived*>(b.get());
    check(d != nullptr && d->x == 7, "dynamic_cast to the derived class");
    const Base& ref = *b;
    check(typeid(ref) == typeid(Derived), "typeid of a polymorphic object");
}

std::atomic<int> tl_constructed{0};
std::atomic<int> tl_destroyed{0};
struct PerThread {
    int id;
    PerThread() : id(++tl_constructed) {}
    ~PerThread() { ++tl_destroyed; }
};
thread_local PerThread per_thread;

void threads() {
    std::mutex m;
    std::condition_variable cv;
    int ready = 0;
    std::atomic<int> sum{0};
    std::vector<std::thread> ts;
    for (int i = 1; i <= 4; i++) {
        ts.emplace_back([&, i] {
            (void)per_thread.id;  // this thread's, constructed on first use
            sum += i;
            std::lock_guard<std::mutex> l(m);
            ready++;
            cv.notify_one();
        });
    }
    {
        std::unique_lock<std::mutex> l(m);
        cv.wait(l, [&] { return ready == 4; });
    }
    for (auto& t : ts) {
        t.join();
    }
    check(sum == 10, "four threads, a mutex and a condition variable");
    check(tl_constructed == 4 && tl_destroyed == 4, "thread_local constructed and destroyed per thread");

    std::atomic<int> flag{0};
    std::thread waker([&] {
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
        flag.store(1);
        flag.notify_one();
    });
    flag.wait(0);
    waker.join();
    check(flag.load() == 1, "atomic wait and notify");
}

void library() {
    std::map<std::string, int> m{{"b", 2}, {"a", 1}};
    std::unordered_map<int, std::string> u{{3, "three"}};
    check(m.begin()->first == "a" && u.at(3) == "three", "map and unordered_map");

    std::ostringstream os;
    os << "pi=" << 3.25 << ' ' << std::hex << 255;
    check(os.str() == "pi=3.25 ff", "an ostringstream");

    check(std::format("{:>5}|{:.2f}|{}", "ab", 2.5, 42) == "   ab|2.50|42", "std::format");

    char buf[32];
    auto r = std::to_chars(buf, buf + sizeof buf, 1.5e10);
    check(std::string(buf, r.ptr) == "1.5e+10", "to_chars of a double");  // the shorter form

    namespace fs = std::filesystem;
    std::error_code ec;
    fs::path dir = fs::temp_directory_path(ec) / "cxx_smoke";
    fs::remove_all(dir, ec);
    bool made = fs::create_directories(dir / "sub", ec);
    { std::FILE* f = std::fopen((dir / "f.txt").c_str(), "w"); if (f) { std::fputs("12345", f); std::fclose(f); } }
    int n = 0;
    for (auto& e : fs::directory_iterator(dir, ec)) {
        (void)e;
        n++;
    }
    bool sized = fs::file_size(dir / "f.txt", ec) == 5;
    fs::remove_all(dir, ec);
    check(made && n == 2 && sized && !fs::exists(dir), "std::filesystem");

    auto t0 = std::chrono::steady_clock::now();
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
    check(std::chrono::steady_clock::now() - t0 >= std::chrono::milliseconds(10), "steady_clock and sleep_for");

    std::random_device rd;
    std::mt19937 gen(rd());
    std::uniform_int_distribution<int> dist(1, 6);
    int roll = dist(gen);
    check(roll >= 1 && roll <= 6, "random_device and a distribution");
}

}  // namespace

int main() {
    check(early.value == 42, "a global's constructor before main");
    exceptions();
    rtti();
    threads();
    library();
    if (failed != nullptr) {
        std::printf("[ FAIL ] cxx: %s\n", failed);
        return 1;
    }
    std::printf("[ OK ] cxx\n");
    return 0;
}
