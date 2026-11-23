import json, random, time, urllib.request, sys, datetime
URL="http://127.0.0.1:5399/ingest/clef"; RATE=int(sys.argv[1]); SECS=int(sys.argv[2])
lv=["Information"]*7+["Debug"]*2+["Warning","Error"]
tpl=["Request {Method} {Path} responded {Status} in {Elapsed} ms","User {UserId} logged in from {Ip}",
     "Order {OrderId} placed for {Amount} {Currency}","Cache miss for {Key}","Job {Job} failed: {Reason}"]
def ev():
    t=random.choice(tpl)
    return {"@t":datetime.datetime.now(datetime.timezone.utc).isoformat(),"@mt":t,"@l":random.choice(lv),
      "Method":random.choice(["GET","POST"]),"Path":f"/api/v1/items/{random.randint(1,10**6)}","Status":random.choice([200,200,201,404,500]),
      "Elapsed":round(random.random()*900,2),"UserId":random.randint(1,50000),"Ip":f"10.0.{random.randint(0,255)}.{random.randint(0,255)}",
      "OrderId":random.randint(1,10**8),"Amount":round(random.random()*500,2),"Currency":"EUR","Key":f"k{random.randint(1,10**7)}",
      "Job":random.choice(["sync","mail","report"]),"Reason":"timeout after 30s","Service":random.choice(["py-portal","net-portal","api"]),
      "Payload":"x"*random.randint(50,400)}
sent=0; start=time.time(); batch=500
while time.time()-start<SECS:
    t0=time.time(); n=max(1,RATE//(1000//batch) if RATE>=batch else RATE)
    for _ in range(max(1,RATE//batch)):
        body="\n".join(json.dumps(ev()) for _ in range(batch)).encode()
        urllib.request.urlopen(urllib.request.Request(URL,body,{"Content-Type":"application/vnd.serilog.clef"})).read(); sent+=batch
    d=time.time()-t0
    if d<1: time.sleep(1-d)
print(f"sent {sent} events in {time.time()-start:.0f}s ({sent/(time.time()-start):.0f}/s)")
