from sys import stderr


for i in range(1000000):
  print('set name'+str(i),'helloworld')
  print(i, file=stderr)
