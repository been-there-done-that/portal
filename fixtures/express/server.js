const http = require('http');
const port = process.env.PORT || 3000;
http.createServer((req, res) => {
  res.writeHead(200, { 'Content-Type': 'text/plain' });
  res.end('hello from express fixture');
}).listen(port, () => {
  console.log(`listening on port ${port}`);
});
