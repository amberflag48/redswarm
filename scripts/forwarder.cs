// TCP forwarder for WSL2 port forwarding.
// Listens on 0.0.0.0:6882 and forwards to 127.0.0.1:6882 (WSL2 mirrored mode).
// Run with: dotnet run -- or compile with csc and run the .exe.
// Or run via PowerShell: Add-Type -TypeDefinition (Get-Content -Raw forwarder.cs) -OutputAssembly forwarder.exe -OutputType ConsoleApplication
// Then: .\forwarder.exe

using System;
using System.Net;
using System.Net.Sockets;
using System.Threading;

class Forwarder
{
    static void Main(string[] args)
    {
        int listenPort = 6882;
        string forwardHost = "127.0.0.1";
        int forwardPort = 16882;

        if (args.Length >= 1) listenPort = int.Parse(args[0]);
        if (args.Length >= 3) { forwardHost = args[1]; forwardPort = int.Parse(args[2]); }

        var listener = new TcpListener(IPAddress.Any, listenPort);
        listener.Start();
        Console.WriteLine("Forwarding 0.0.0.0:" + listenPort + " -> " + forwardHost + ":" + forwardPort);

        while (true)
        {
            var client = listener.AcceptTcpClient();
            Console.WriteLine("Connection from " + client.Client.RemoteEndPoint);
            var thread = new Thread(() => HandleClient(client, forwardHost, forwardPort));
            thread.IsBackground = true;
            thread.Start();
        }
    }

    static void HandleClient(TcpClient client, string host, int port)
    {
        try
        {
            var remote = new TcpClient(host, port);
            var cs = client.GetStream();
            var rs = remote.GetStream();

            var t1 = new Thread(() => Pipe(cs, rs));
            var t2 = new Thread(() => Pipe(rs, cs));
            t1.IsBackground = true;
            t2.IsBackground = true;
            t1.Start();
            t2.Start();
            t1.Join();
            t2.Join();

            client.Close();
            remote.Close();
        }
        catch (Exception e)
        {
            Console.WriteLine("Error: " + e.Message);
            try { client.Close(); } catch {}
        }
    }

    static void Pipe(NetworkStream input, NetworkStream output)
    {
        var buffer = new byte[65536];
        int count;
        while ((count = input.Read(buffer, 0, buffer.Length)) > 0)
        {
            output.Write(buffer, 0, count);
        }
        try { input.Close(); } catch {}
        try { output.Close(); } catch {}
    }
}
