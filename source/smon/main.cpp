
#include <iostream>
#include <thread>

#include <boost/filesystem.hpp>

using namespace std;

int main() {
  cout << "Hello spes" << endl;

  cout << boost::filesystem::exists("spes") << endl;
  
  return 0;
}
